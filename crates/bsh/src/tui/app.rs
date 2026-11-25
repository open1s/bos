//! Client-side state and renderer of the terminal interface.
//!
//! Every field here is derived from the Host: the session list, the open
//! transcript and the approvals all come from Queries, and every action is a
//! Command or Control. Nothing in this file can mark a turn finished, retry a
//! failed one or wake an executor - a client only describes what the Host
//! already decided.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

use chrono::{DateTime, Utc};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::runtime::client::{ClientError, RuntimeClient, tui_info};
use crate::runtime::contract::{
    ApprovalStatus, ApprovalView, BrowserElementView, BrowserView, CatalogueView, CommandOutcome,
    Cursor, EventPayload, ExecutionActivity, ExecutionStatus, ExecutionView, ExtensionView,
    ExtensionsReport, FederationGrantView, FederationPeerView, FederationReport, GoalState,
    HostStatus, McpServerView, MemoryReport, MemoryView, MessageRole, MessageView,
    PERMISSION_MODES, PlanItemStatus, PlanItemView, QueryPayload, QueryResult, Response,
    ReviewChangeView, ReviewDiffView, ReviewReport, ServerFrame, SessionConfig,
    SessionConfigChange, SessionProjection, SessionSummary, SettingEdit, SettingsReport,
    SettingsRowView, SettingsSectionView, SkillView, SubagentReport, SubagentRunView,
    SubscriptionClosedReason, TaskAttemptView, TaskBoardReport, TaskDetailReport, TaskRowView,
    ToolCallStatus, ToolCallView, TurnUsage, WorkspaceChangeView, WorkspaceDiffView,
    WorkspaceEntryView, WorkspaceReport,
};
use crate::runtime::ids::{ExecutionId, SessionId};

use super::export;
use super::menu;
use super::picker;
use super::shell::{self, Shell};
use super::skin::{self, Footer, Painter, Skin, Tone};
use super::term::{self, Key};

/// Lines one page key moves through the transcript. The client paints against
/// the cells it is about to fill, so a page is a fixed step here rather than a
/// measurement of the last frame.
const PAGE: usize = 10;

/// Lines one wheel notch moves through the transcript. A notch is a small
/// step - the reader is aiming at a line, not turning a page.
const SCROLL: usize = 3;

/// Painted frames between two re-reads of an open child-run roster.
///
/// A child's state changes inside the Host's process and no committed event
/// carries it here, so the pane asks again; asking on every frame would spend a
/// query per paint for a roster that moves on the scale of a turn.
const SUBAGENT_POLL_EVERY: u32 = 15;

/// The command reference `/help` shows in the status row.
const COMMANDS: &str = "commands: /new [title] · /sessions [number|text] · \
                        /sessions pin|unpin|archive|unarchive <number> · \
                        /sessions archived · \
                        /model [provider model] · /preset [id] · /permission [mode] · \
                        (bare picks from a list) · /plan [on|off] · /goal <objective> · \
                        /goal edit|pause|resume|clear · /compact · /fork · \
                        /copy [number|id] · \
                        /rename <title> · /editor · \
                        /export [path] · /queue [clear] · /feedback [note|clear] · \
                        /detail [number|close] · /mcp [server] · /skills [text] · \
                        /memory [text] · /files [path] · /diff [path] · /extensions [text] · \
                        /tasks [id|graph] · /settings [section] · /set [key] [value] · \
                        /federation · /browser · /subagents [n|all] · /trajectory · /audit [filter] · \
                        /find <text> · /review [change] · /questions · /trash [number] · \
                        /restore <number> · /purge <number> · /usage · /terminal · \
                        /approve · /deny [reason] — /help";

/// Wide layouts get a session sidebar; narrow ones show only the transcript.
const SIDEBAR_MIN_WIDTH: u16 = 64;

/// The rows above the body: the header line and the rule under it.
///
/// The rail's click hit test measures from here, so it is named once rather
/// than counted at two call sites.
const BODY_TOP: usize = 2;

/// The session rail's width, when the frame is wide enough for two columns.
///
/// The frame paints the rail with this width and the pointer hit-tests
/// against it, so a click can never name a column the rail does not own.
fn rail_width(width: usize) -> Option<usize> {
    (width >= usize::from(SIDEBAR_MIN_WIDTH)).then(|| (width / 4).clamp(18, 30))
}

/// The rail row a click at `(row, column)` names, as an offset into the list
/// window the frame painted, or `None` when the click missed the rail.
///
/// The separator between the columns belongs to neither: it is the rule the
/// transcript starts against, so a click on it is not a session.
fn rail_offset(row: usize, column: usize, width: usize, body_rows: usize) -> Option<usize> {
    let rail = rail_width(width)?;
    if column >= rail || row < BODY_TOP || row >= BODY_TOP + body_rows {
        return None;
    }
    Some(row - BODY_TOP)
}

/// The transcript column's width: the frame minus the rail and its separator.
///
/// The painter and the pointer both measure the column here, so a click can
/// never name a row the transcript does not paint.
fn transcript_width(width: usize) -> usize {
    match rail_width(width) {
        Some(left) => width.saturating_sub(left + 1),
        None => width,
    }
}

/// The transcript row a click at `(row, column)` names, as an offset into the
/// document the frame painted, or `None` when the click missed the column.
///
/// The separator between the two columns belongs to neither, and the rows
/// above the body are chrome: neither names a tool call. A frame with no width
/// has painted no column at all, so nothing there is a row either.
fn transcript_offset(row: usize, column: usize, width: usize, body_rows: usize) -> Option<usize> {
    if row < BODY_TOP || row >= BODY_TOP + body_rows {
        return None;
    }
    let left = match rail_width(width) {
        Some(rail) => rail + 1,
        None => 0,
    };
    // The column has to fall inside the transcript's own cells: left of it is
    // the rail or its separator, and past it is a column the frame never
    // painted.
    if column < left || column >= left + transcript_width(width) {
        return None;
    }
    Some(row - BODY_TOP)
}

/// One block of transcript rows, each naming the tool call it draws.
fn targets(rows: Vec<String>, call: Option<usize>) -> Vec<(String, Option<usize>)> {
    rows.into_iter().map(|row| (row, call)).collect()
}

/// The choice row a click on screen row `row` names, or `None` when the click
/// missed every choice.
///
/// A list is painted from `top` down: first `header` rows that belong to the
/// list but name nothing (the choice list's title and key line), then the rows
/// in `start..start + visible`. The pointer's row is the slot it stands in, so a
/// click on a header row, or past the last row the list drew, names no choice -
/// which is why a filter that hides rows can never make a click pick a row that
/// is not drawn.
fn list_offset(
    row: usize,
    top: usize,
    header: usize,
    start: usize,
    visible: usize,
    shown: usize,
) -> Option<usize> {
    let slot = row.checked_sub(top)?.checked_sub(header)?;
    if slot >= visible {
        return None;
    }
    let index = start + slot;
    (index < shown).then_some(index)
}

/// The rows of a transcript a body `rows` tall shows, and the tool call each
/// shown row names.
///
/// The document is bottom-aligned: its newest row sits directly above the
/// input row, and `scroll` lifts older rows into view. The hits are cut with
/// the same window, so a click opens the call the frame painted there rather
/// than the call at that index before the scroll.
fn window_bounds(total: usize, rows: usize, scroll: usize) -> (usize, usize) {
    let end = total.saturating_sub(scroll.min(total));
    (end.saturating_sub(rows), end)
}

fn window_rows(
    lines: &[(String, Option<usize>)],
    rows: usize,
    scroll: usize,
) -> (Vec<String>, Vec<Option<usize>>) {
    let (start, end) = window_bounds(lines.len(), rows, scroll);
    let fill = rows.saturating_sub(end - start);
    let mut texts = vec![String::new(); fill];
    let mut hits = vec![None; fill];
    for (text, call) in &lines[start..end] {
        texts.push(text.clone());
        hits.push(*call);
    }
    (texts, hits)
}

/// A styled frame: the lines a pane draws, and the cursor cell to place when the
/// pane has one.
///
/// Every other document in this client is plain text; a shell's grid carries a
/// colour per cell, so its frame cannot be.
type ShellFrame = (Vec<Line<'static>>, Option<(usize, usize)>);

/// How many composer rows a tall prompt may show before it scrolls.
const MAX_COMPOSER_ROWS: usize = 6;

/// How many plan steps the block above the composer draws before it folds the
/// rest into one counted row: the panel belongs above the prompt, so it may
/// never take the transcript's rows for itself.
const MAX_PLAN_ROWS: usize = 6;

/// How many changed rows of a landed patch a transcript row draws.
const PATCH_ROWS: usize = 10;

/// How many rows one tool call's detail may draw before it folds the rest into
/// one counted row.
///
/// The desktop's own detail panes cap at [`crate::ui::diff_pane::TURN_MAX_ROWS`]
/// for the same reason: a call can carry a whole file, and a client that paints
/// thousands of rows per frame stops being a terminal. Unlike the transcript's
/// [`PATCH_ROWS`], this budget is the reader's explicit request to see the call,
/// so it is generous.
const DETAIL_ROWS: usize = 400;

/// How deep the Tasks graph nests one parent chain before it folds the rest.
///
/// A record's `parent` is model-written, so a long chain must not be able to
/// grow one frame's work without a bound; the fold is counted, never silent.
const GRAPH_DEPTH: usize = 12;

/// How many characters of one remembered text a Memory pane row carries.
///
/// The desktop card truncates each record at the same figure: a memory can
/// hold a whole document, and a terminal must not paint it as one transcript
/// row.
const MEMORY_CONTENT_CHARS: usize = 280;

/// How many queued lines the dock above the composer previews before folding
/// the rest into one counted row.
const MAX_QUEUE_ROWS: usize = 3;

/// How many characters of one queued line its preview keeps, the same budget
/// the desktop's `previewOf` spends before it adds its ellipsis.
const QUEUE_PREVIEW_CHARS: usize = 200;

/// How many options of one question the card draws.
///
/// The tool refuses more than six, so this is the tool's own ceiling rather
/// than a terminal budget: every option the model offered is on screen.
const MAX_QUESTION_OPTIONS: usize = 6;

/// Severity of the last status message.
#[derive(Debug, Clone, Copy)]
enum Level {
    Info,
    Error,
}

/// The terminal client's whole state.
pub(crate) struct App {
    client: RuntimeClient,
    /// The Host's data directory. `/export` writes under it when the reader
    /// names no path, so a terminal that never asks leaves a conversation
    /// where the next export - or the desktop - can find it.
    data_dir: PathBuf,
    frames: Receiver<ServerFrame>,
    sessions: Vec<SessionSummary>,
    /// The workspace's bin: the parked sessions the last `/trash` listed, in
    /// the order the numbers `/restore` and `/purge` address. It is client
    /// state because a bin is a document the reader opens: the Host owns the
    /// records, this holds only the snapshot of them that is on screen.
    trash: Vec<SessionSummary>,
    /// The workspace's Archived filter: the rows the last `/sessions archived`
    /// listed, in the order `/sessions unarchive` addresses. Like the bin it
    /// is a document the reader opens, so this holds only the snapshot that is
    /// on screen while the Host owns the records.
    archived: Vec<SessionSummary>,
    selected: usize,
    session: Option<SessionProjection>,
    /// Models and presets the Host offers; the terminal's form of the desktop
    /// pickers, refreshed when the client connects.
    catalogue: CatalogueView,
    host: Option<HostStatus>,
    status: Option<(Level, String)>,
    /// How this terminal paints: the hues the reader's profile accepts,
    /// resolved once when the client attaches so every frame draws the same
    /// vocabulary.
    skin: Skin,
    /// The size of the frame last painted.
    ///
    /// A click arrives in screen cells, and only the layout that painted those
    /// cells knows which row is which session. Recording the size here lets the
    /// pointer use the same arithmetic as the frame rather than a guess at the
    /// terminal's dimensions.
    frame: Cell<(usize, usize)>,
    input: String,
    /// Lines this client submitted, recalled with Up/Down the way a shell
    /// recalls them.
    history: PromptHistory,
    /// Byte offset in `input`, always on a character boundary.
    cursor: usize,
    /// Lines scrolled up from the newest transcript line.
    scroll: usize,
    /// The tool call the reader opened for detail, as an index into the
    /// projection's `tool_calls`. The pane replaces the transcript's rows;
    /// `None` is the ordinary timeline.
    detail: Option<usize>,
    /// Rows scrolled down from the first row of the open detail pane. A detail
    /// is a document the reader asked for, so it opens at its top; the offset
    /// walks towards the end of a long output.
    detail_scroll: usize,
    /// The workspace's durable memories, when the reader asked to browse or
    /// search them. Like the detail pane it replaces the transcript's rows;
    /// `None` is the ordinary timeline.
    memory: Option<MemoryPane>,
    /// The workspace the Host serves, when the reader asked to browse it or to
    /// read one file in it. Like the memory pane it replaces the transcript's
    /// rows; `None` is the ordinary timeline.
    files: Option<FilesPane>,
    /// The working copy's comparison, when the reader asked for it: the
    /// desktop Diff tab. Like every other pane it replaces the transcript's
    /// rows; `None` is the ordinary timeline.
    diff: Option<DiffPane>,
    /// The workspace's durable task board, when the reader asked for it. Read
    /// from the Host on open, because a turn's task tools write those records
    /// while this client is attached.
    tasks: Option<TasksPane>,
    /// The conversation's audited tool calls, when the reader asked to see
    /// them. Derived from the projection, so opening it sends no request.
    audit: Option<AuditPane>,
    /// The extensions this build installed, when the reader asked to see them.
    /// Read from the Host on open, because a package can be installed or
    /// switched off while this client is attached.
    extensions: Option<ExtensionsPane>,
    /// The open session's latest sealed review, when the reader asked for it.
    /// Read from the Host on open, because a turn running beside this client
    /// seals its changeset at any moment.
    review: Option<ReviewPane>,
    /// The federation node this build runs and the peers it trusts, when the
    /// reader asked to see them. Read from the Host on open, because a peer can
    /// be enrolled or revoked while this client is attached.
    federation: Option<FederationPane>,
    /// The browser's page: the desktop's Browser tab, in the form a terminal
    /// can read. Read from the Host on open, because the turn running beside
    /// this client may visit a page at any moment.
    browser: Option<BrowserPane>,
    /// The conversation's delegated child runs, when the reader asked for
    /// them: the desktop's task-graph children and header list. Read from the
    /// Host on open and re-read while a child still runs, because a run
    /// finishes without any event this client subscribed to.
    subagents: Option<SubagentsPane>,
    /// The open session's trajectory, when the reader asked for it: every turn
    /// the Host ran with the messages and calls it produced. Derived from the
    /// projection, so opening it sends no request.
    trajectory: Option<TrajectoryPane>,
    /// The client's own shell, when the reader opened the terminal pane. A
    /// terminal has no Ghostty surface to draw into, so this pane owns the pty
    /// and paints its grid into the frame; `None` is the ordinary view.
    shell: Option<Shell>,
    /// The prompt waiting for the reader's own editor, when `/editor` or `^X`
    /// asked for it.
    ///
    /// The event loop owns the terminal, so the app cannot open an editor
    /// itself: it leaves the text here, the loop hands the screen over, and the
    /// answer comes back through [`App::after_edit`].
    edit_request: Option<String>,
    /// What the Host's settings resolve to, when the reader asked to read them.
    /// The desktop's settings dialog is the editor; this pane is its read side,
    /// as a terminal can show it, so a reader can see what a turn runs with.
    settings: Option<SettingsPane>,
    /// Journal position of the last event this client applied.
    applied: u64,
    subscription: Option<u64>,
    /// Assistant text streamed for a running turn. The Host never journals it,
    /// so it is replaced by the committed message when that turn lands.
    live: Option<LiveText>,
    /// The workspace the `@` list ranks. It comes from the same configuration
    /// the Host was handed, so the terminal offers the folder the desktop
    /// window opens rather than whatever directory the shell inherited.
    workspace: Option<PathBuf>,
    /// The `/` and `@` completion list.
    menu: menu::Menu,
    /// The list a choice-taking command opened: `/model`, `/preset` and
    /// `/permission` answer a reader who typed only the command with the
    /// desktop's own picker, instead of a row of names to retype from.
    picker: Option<picker::Picker>,
    /// The screen row the open choice list's title was painted on.
    ///
    /// A click names a screen row, and only the frame that painted the list
    /// knows which of its rows that is, so the frame records the block's top
    /// here the same way it records its size in `frame`.
    picker_at: Cell<Option<usize>>,
    /// The screen row the open completion popup's first row was painted on.
    ///
    /// The popup has no title: its rows are candidates, so the frame records
    /// the row its first candidate stood on and a click reads it back.
    menu_at: Cell<Option<usize>>,
    /// The document row the body's first painted row stands for, while the
    /// body is the timeline.
    ///
    /// A `/find` hit is a document row, and the frame paints a window of the
    /// document, so the offset between the two is the only way the frame can
    /// reverse the row the reader picked. A pane scrolls its own way, so it
    /// records nothing here and the highlight simply is not painted.
    body_start: Cell<Option<usize>>,
    /// The document row a `/find` revealed, reversed for as long as it is on
    /// screen. Cleared when the conversation it numbered is replaced.
    focus: Option<usize>,
    /// The rows the last painted frame drew in the transcript column.
    ///
    /// `/find` searches exactly what the reader can see: the wrapping, the
    /// tones and the call each row names are the frame's, so a hit is a row of
    /// the screen and not a row of some other layout. A pane paints itself
    /// instead, so this is empty while one is open and no row is named.
    document: RefCell<Vec<(String, Option<usize>)>>,
    /// Turns typed while the open session was running, waiting their turn.
    queue: DraftQueue,
    /// The ask_user card, when a turn asked questions this client has not
    /// settled. Client state: the projection carries the call's arguments and
    /// the answers leave as an ordinary turn, so nothing here is durable.
    question: Option<QuestionCard>,
    /// The call this client put away with Escape, so a refresh does not bring
    /// the card back under the reader's hands.
    question_dismissed: Option<usize>,
    /// Whether this client is mid-dispatch of one queued line; a second
    /// dispatch from a nested refresh would race that line.
    flushing: bool,
    quit: bool,
}

/// The answer a running turn has streamed so far.
///
/// It is keyed by session and execution because activity arrives for whichever
/// turn the Host is running, and the open session only shows its own.
struct LiveText {
    session: SessionId,
    execution: ExecutionId,
    text: String,
}

/// One question an `ask_user` call asked, as the card draws it.
///
/// The shape is the tool's own - the model passes `question`, `options` and
/// `multi_select` - so a client shows what the model asked rather than a
/// second reading of it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AskedQuestion {
    /// The question, in the model's own words.
    question: String,
    /// The choices it offered, in order.
    options: Vec<String>,
    /// Whether several options may be picked at one step.
    multi_select: bool,
}

/// The ask_user card: the questions one turn asked, and the picks so far.
///
/// The desktop draws the same questions as a card in the transcript and sends
/// the reader's choices as the next user message. A terminal has no buttons,
/// so the card sits above the prompt and walks one step at a time: Up/Down walk
/// the options, Enter picks, Ctrl-J moves a multi-select step on, Escape puts it
/// away. The text it sends is the desktop's own shape - `question: pick, pick`
/// per step - so the model receives the same message whichever interface asked.
#[derive(Debug, Clone, PartialEq, Eq)]
struct QuestionCard {
    /// The call's index in the projection: what keeps one card per call and
    /// keeps a set-aside card away.
    call: usize,
    /// The steps, in the order the model asked them.
    questions: Vec<AskedQuestion>,
    /// The step the reader is on.
    step: usize,
    /// The picks made at each step; one entry per step, empty until picked.
    picks: Vec<Vec<String>>,
    /// The highlighted option at the current step.
    highlight: usize,
}

impl QuestionCard {
    /// A fresh card, on its first step with nothing picked.
    fn new(call: usize, questions: Vec<AskedQuestion>) -> Self {
        let picks = vec![Vec::new(); questions.len()];
        Self {
            call,
            questions,
            step: 0,
            picks,
            highlight: 0,
        }
    }

    /// The step the reader is on, or `None` for a card with no questions.
    fn current(&self) -> Option<&AskedQuestion> {
        self.questions.get(self.step)
    }

    /// Walk the highlight, clamped at both ends: the first press never wraps
    /// to the last option the way a menu's wheel would.
    fn move_by(&mut self, delta: isize) {
        let Some(question) = self.current() else {
            return;
        };
        let last = question.options.len().saturating_sub(1);
        self.highlight = self.highlight.saturating_add_signed(delta).min(last);
    }

    /// Put the highlight on the option a pressed digit names, one-based.
    ///
    /// A digit past the last option moves nothing: the row a reader aimed at
    /// does not exist, and guessing a neighbour would be worse than ignoring
    /// the press.
    fn jump(&mut self, digit: char) {
        let Some(question) = self.current() else {
            return;
        };
        let Some(one_based) = digit.to_digit(10).filter(|digit| *digit > 0) else {
            return;
        };
        let index = one_based as usize - 1;
        if index < question.options.len() {
            self.highlight = index;
        }
    }

    /// Take the highlighted option. Returns whether the step is multi-select,
    /// which is what tells the caller to toggle and wait rather than advance.
    ///
    /// A multi-select step toggles the option in and out; a single-select step
    /// replaces its one pick, because the desktop's card also lets a reader
    /// change their mind before the last step sends.
    fn pick(&mut self) -> Option<bool> {
        let step = self.step;
        let multi = self.questions.get(step)?.multi_select;
        let option = self
            .questions
            .get(step)?
            .options
            .get(self.highlight)?
            .clone();
        let picks = self.picks.get_mut(step)?;
        if multi {
            if let Some(position) = picks.iter().position(|pick| pick == &option) {
                picks.remove(position);
            } else {
                picks.push(option);
            }
        } else {
            *picks = vec![option];
        }
        Some(multi)
    }

    /// Move to the next step, or report that this was the last one.
    fn advance(&mut self) -> bool {
        if self.step + 1 >= self.questions.len() {
            return true;
        }
        self.step += 1;
        self.highlight = 0;
        false
    }

    /// The message the picks become: one `question: pick, pick` line per
    /// step, exactly as the desktop's card composes it.
    fn answer(&self) -> String {
        self.questions
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let picks = self.picks.get(index).cloned().unwrap_or_default();
                format!("{}: {}", item.question, picks.join(", "))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// The terminal's Memory pane: what the workspace's durable store holds.
///
/// The desktop's Memory pane is the reader's surface over the Durable Record
/// Kernel. A terminal has the same store behind the same Host query, so this
/// pane lists the same rows - text, kind, scope, lifecycle, revision and age -
/// and runs the same hybrid recall when the reader types a search. Like the
/// detail pane it replaces the transcript's rows, and its own title row keeps
/// "what am I looking at?" answered while it scrolls.
struct MemoryPane {
    /// The report the Host answered with: records newest first when listing,
    /// best first when searching.
    report: MemoryReport,
    /// The search this report answers; `None` lists the store.
    query: Option<String>,
    /// Rows scrolled down from the pane's first body row.
    scroll: usize,
}

/// The terminal's Diff pane: the desktop's Diff tab.
///
/// The window opens that tab two ways - on the working copy's changed files,
/// and on one file's comparison - and a terminal carries the same two: the
/// rows are the changed paths with the letter the Files tree badges them with,
/// Enter opens the highlighted row's unified patch, and a typed path opens one
/// directly. Nothing here computes a diff: the Host asks the VCS and this pane
/// draws what it answered, so a terminal and a window cannot disagree about
/// what changed.
struct DiffPane {
    /// What the Host answered: the change list, one patch, or the reason it
    /// could not be either.
    view: WorkspaceDiffView,
    /// The highlighted change row, clamped when it is read.
    selection: usize,
    /// Rows scrolled down from the pane's first body row, in patch mode.
    scroll: usize,
}

/// The terminal's Workspace Files pane: the desktop's right-panel browser.
///
/// The desktop lists one directory at a time and opens a file in its editor
/// area; a terminal has the same tree behind the same Host query, so this pane
/// lists one directory - directories first - and pages one file's text. Item
/// selection is the terminal's pointer: Up and Down walk the rows and Enter
/// descends, which is the one interaction a click cannot carry here.
struct FilesPane {
    /// What the Host answered: a listing, a document, or the reason it could
    /// not be either.
    report: WorkspaceReport,
    /// The highlighted listing row. Like every other offset here it is
    /// clamped when it is read, so a shrinking directory cannot point a
    /// selection past its last row.
    selection: usize,
    /// Rows scrolled down from the pane's first body row, in file mode.
    scroll: usize,
}

/// The terminal's Extensions pane: the desktop's settings > extensions list.
///
/// The desktop draws that list as a market with a search box and one card per
/// package carrying its state, its health and what it contributed. A terminal
/// can neither install a package nor switch one on without the confirmation
/// flow that belongs to a window, so this pane is the installed half: the
/// roster the extension host reports, one row per package, with the reason a
/// failed package keeps. That is the same roster the desktop filters with
/// "installed", read from the same host, so the two can never disagree about
/// which tools a turn can call.
struct ExtensionsPane {
    /// What the Host answered: the roster, or why it could not be read.
    report: ExtensionsReport,
    /// The search text the reader typed, when one was given; it filters ids and
    /// display names the way the desktop's search box does.
    filter: Option<String>,
    /// Rows scrolled down from the pane's first body row.
    scroll: usize,
}

/// The terminal's Review pane: the desktop's Review tab.
///
/// The desktop draws a change list and a diff beside it. A terminal has one
/// column, so the list and the diff take turns: a bare `/review` lists the
/// changeset, and naming a change - its id, or the path a reader typed -
/// replaces the list with that file's prepared diff, which is the row a click
/// opens on the desktop.
struct ReviewPane {
    /// What the Host answered: the changeset, its reading, and - when the
    /// request named a change - its prepared diff.
    report: ReviewReport,
    /// The highlighted change, walked by Up/Down while the list is showing.
    /// Clamped when it is read, so a report that shrank cannot point past its
    /// last row.
    selection: usize,
    /// Rows scrolled down from the pane's first body row, in diff mode.
    scroll: usize,
}

/// The terminal's Settings pane: the desktop settings rail, read-only.
///
/// The desktop's settings dialog edits the two documents a turn resolves from;
/// a terminal cannot type into those forms yet, so this pane shows the same
/// resolution the turn uses - the model, its credential source, the workspace
/// root and permission, the tool switches, the search endpoint, the vault and
/// the machine preferences - under the rail's section names. Opening it asks
/// the Host, so the rows are the values the next turn will use rather than a
/// second read of the file.
struct SettingsPane {
    /// What the Host answered: the sections, the state and any refusal.
    report: SettingsReport,
    /// The section the reader named, when they narrowed to one.
    section: Option<String>,
    /// Rows scrolled down from the pane's first body row.
    scroll: usize,
}

/// The terminal's Tasks pane: the desktop's durable task board.
///
/// The desktop's right panel draws four sections - running, queued, scheduled
/// and history - from the workspace's task records, and the Host projects the
/// same records through the same board function, so this pane lists the sections
/// a desktop window lists rather than a second opinion about what is running.
/// The read is a Host query on open: a turn's task tools write those records
/// while the client is attached, so a cached board would show the work from
/// before the turn that changed it.
struct TasksPane {
    /// The report the Host answered with: the four sections and their rows.
    report: TaskBoardReport,
    /// One task opened from the board, when a reader named an id.
    detail: Option<TaskDetailReport>,
    /// `/tasks graph` opened the desktop's graph tab: the rows nested under the
    /// parents they name, with each row's dependencies spelled out, instead of
    /// the four board sections.
    graph: bool,
    /// Rows scrolled down from the pane's first body row.
    scroll: usize,
}

/// The terminal's Federation pane: the desktop's Federation tab.
///
/// The desktop draws a hero row naming the node, then one card per trusted peer
/// and the capabilities each may invoke. A terminal has the same two facts
/// behind the same Host query - the node id and name, and the roster with its
/// grants - so this pane lists the peers a window lists rather than a second
/// opinion about who is trusted for what.
struct FederationPane {
    /// What the Host answered: the node, its roster, or why neither could be
    /// read.
    report: FederationReport,
    /// Rows scrolled down from the pane's first body row.
    scroll: usize,
}

/// The conversation's delegated child runs: the desktop's task-graph children.
///
/// The desktop draws these as nodes under the conversation in its Tasks graph
/// and as rows in the header, from the in-process registry the `subagent` tool
/// writes. A client that attached to the Host cannot see that registry, so the
/// Host answers for it and this pane draws the same rows: the title the model
/// gave the child, its state, its depth, its token use and its answer. A run
/// outlives the turn that started it, so the pane is read from the Host rather
/// than counted from the transcript, and re-read while a child still works.
struct SubagentsPane {
    /// What the Host answered: the runs, or the state that explains none.
    report: SubagentReport,
    /// The highlighted row, clamped where it is read so a roster that shrinks
    /// under a live run cannot point past its last row.
    selection: usize,
    /// The run the reader opened, as an index into `report.runs`: its prompt
    /// and its answer, which a terminal has no hover card to show.
    open: Option<usize>,
    /// Rows scrolled down from the pane's first body row.
    scroll: usize,
    /// Paints since the last re-read. A child's state changes without a Host
    /// event this client subscribed to, so a live pane asks again on a slow
    /// tick instead of on every frame or never.
    ticks: u32,
    /// The conversation whose lineage this roster was read for: `None` is the
    /// Host's whole roster, which is what `/subagents all` asks for. Kept so a
    /// re-read asks the same question the pane was opened with.
    scope: Option<SessionId>,
}

/// The terminal's Browser pane: the desktop's Browser tab.
///
/// The desktop pane is a text view of the page the agent's browser tools read:
/// the address bar, the navigation state, the loading or failure line, and the
/// extracted title, URL, text and interactive elements. A terminal draws the
/// same facts from the same Host query, so the page a turn read is the page the
/// reader sees - no terminal here paints a page the browser did not load.
struct BrowserPane {
    /// What the Host answered: the page, or why the browser cannot be read.
    view: BrowserView,
    /// Rows scrolled down from the pane's first body row.
    scroll: usize,
}

/// The session's trajectory, open when the reader asked to see it.
///
/// Unlike the federation pane this is derived from the projection the client
/// already holds, so opening it costs nothing and it grows as the turn it
/// describes streams in. Scrolling is measured from the newest row, the way a
/// log's tail works: `back` is how many rows above the end the window sits, and
/// zero means the pane follows the live end of the trajectory.
struct TrajectoryPane {
    /// Rows scrolled up from the newest row; zero follows the tail.
    back: usize,
}

/// The terminal's Audit pane: what this conversation actually did.
///
/// The desktop's AUDIT page is derived, not stored: it is counted from the
/// session's own tool-call rows. The terminal holds the same projection, so this
/// pane answers the same question with no Host query at all - the category,
/// target, outcome and cost of every call, plus the approvals an operator
/// refused. Like the memory and detail panes it replaces the transcript's rows.
struct AuditPane {
    /// Text this pane filters on; `None` lists every call.
    filter: Option<String>,
    /// Rows scrolled down from the pane's first body row.
    scroll: usize,
}

impl App {
    /// Discover the Host at `data_dir`, register, snapshot and subscribe.
    pub(crate) fn connect(
        data_dir: &Path,
        open: Option<SessionId>,
        workspace: Option<PathBuf>,
    ) -> Result<Self, ClientError> {
        let (client, frames) = RuntimeClient::connect(data_dir, tui_info())?;
        let mut app = Self {
            client,
            data_dir: data_dir.to_path_buf(),
            frames,
            sessions: Vec::new(),
            trash: Vec::new(),
            archived: Vec::new(),
            selected: 0,
            session: None,
            catalogue: CatalogueView::default(),
            host: None,
            status: None,
            skin: Skin::detect(),
            frame: Cell::new((0, 0)),
            input: String::new(),
            history: PromptHistory::default(),
            cursor: 0,
            scroll: 0,
            detail: None,
            detail_scroll: 0,
            memory: None,
            files: None,
            diff: None,
            tasks: None,
            review: None,
            audit: None,
            extensions: None,
            federation: None,
            browser: None,
            subagents: None,
            trajectory: None,
            shell: None,
            edit_request: None,
            settings: None,
            applied: 0,
            subscription: None,
            live: None,
            workspace,
            menu: menu::Menu::default(),
            picker: None,
            picker_at: Cell::new(None),
            menu_at: Cell::new(None),
            body_start: Cell::new(None),
            focus: None,
            document: RefCell::new(Vec::new()),
            queue: DraftQueue::default(),
            question: None,
            question_dismissed: None,
            flushing: false,
            quit: false,
        };
        // Snapshot first, subscribe second: the head in the snapshot is the
        // position the subscription resumes from, so no committed event can
        // fall between the two calls.
        let QueryResult::Snapshot { snapshot } = app.client.query(None, QueryPayload::Snapshot)?
        else {
            return Err(ClientError::Contract(
                "Host answered a snapshot query with another projection".to_string(),
            ));
        };
        app.applied = snapshot.head;
        app.catalogue = app.client.catalogue()?;
        // The Host names the folder it serves. In the common case that is the
        // directory this terminal was launched in; when the client attached to
        // a Host another process started, the Host's answer is the one that
        // decides where turns run, so `@`, the shell and every workspace read
        // follow it instead of pointing at a tree no turn can reach.
        app.workspace = host_workspace(&app.catalogue.workspace_root, app.workspace.take());
        app.refresh()?;
        app.subscribe()?;
        if let Some(session_id) = open {
            let Some(index) = app
                .sessions
                .iter()
                .position(|session| session.session_id == session_id)
            else {
                return Err(ClientError::Contract(format!(
                    "session {session_id} does not exist on this Host"
                )));
            };
            app.select(index);
        }
        Ok(app)
    }

    /// Whether the user asked to leave the interface.
    pub(crate) fn is_quitting(&self) -> bool {
        self.quit
    }

    /// The prompt waiting for the reader's editor, when a command asked for
    /// one.
    ///
    /// The event loop polls this after every key instead of the app opening the
    /// editor itself: an editor needs the terminal, and the terminal is the
    /// loop's to give away.
    pub(crate) fn take_edit_request(&mut self) -> Option<String> {
        self.edit_request.take()
    }

    /// Take back what the reader's editor left in its buffer.
    ///
    /// `Ok(Some(text))` is the buffer they wrote, and it replaces the prompt
    /// whether they added a line or cut the whole draft. `Ok(None)` is a
    /// cancelled edit - an editor that exited in failure, which is what `:cq`
    /// and a closed window both do - so the prompt stays word for word as it
    /// was. `Err` is an editor that never ran: the reader has to be told,
    /// because an editor that fails to start looks exactly like one that
    /// opened and saved nothing.
    pub(crate) fn after_edit(&mut self, outcome: Result<Option<String>, String>) {
        match outcome {
            Ok(Some(text)) => {
                let lines = text.lines().count();
                self.input = text;
                self.cursor = self.input.len();
                self.sync_menu();
                self.set_status(
                    Level::Info,
                    format!("the editor left {lines} line(s) in the prompt"),
                );
            }
            Ok(None) => self.set_status(Level::Info, "the editor closed; the prompt is unchanged"),
            Err(error) => self.set_status(
                Level::Error,
                format!("the editor could not run: {error} — set $VISUAL or $EDITOR"),
            ),
        }
    }

    /// Apply every event the Host has pushed since the last call.
    pub(crate) fn pump(&mut self) -> Result<(), ClientError> {
        let mut changed = false;
        let mut closed: Option<SubscriptionClosedReason> = None;
        loop {
            match self.frames.try_recv() {
                Ok(ServerFrame::Event {
                    subscription_id,
                    event,
                }) if self.subscription == Some(subscription_id) => {
                    self.applied = event.sequence;
                    changed = true;
                    // A compaction is a model call the client did not wait on,
                    // so its outcome arrives here: the reader learns why the
                    // transcript moved from the same stream as everything else.
                    match &event.payload {
                        EventPayload::SessionCompacted {
                            summary, folded, ..
                        } => {
                            let head = summary.lines().next().unwrap_or_default();
                            self.set_status(
                                Level::Info,
                                format!("compacted {folded} message(s) into a checkpoint: {head}"),
                            );
                        }
                        EventPayload::SessionCompactionFailed { error, .. } => {
                            self.set_status(
                                Level::Error,
                                format!("compaction failed, history unchanged: {error}"),
                            );
                        }
                        _ => {}
                    }
                }
                Ok(ServerFrame::Event { .. }) => {}
                // A running turn's live answer. It is shown as it arrives and
                // abandoned the moment the turn commits, because the commit
                // carries the whole text and is the only durable truth.
                Ok(ServerFrame::Activity {
                    subscription_id,
                    session_id,
                    execution_id,
                    activity,
                }) if self.subscription == Some(subscription_id)
                    && self.current_session() == Some(session_id) =>
                {
                    let ExecutionActivity::AssistantText { text } = activity;
                    match &mut self.live {
                        Some(live) if live.execution == execution_id => live.text.push_str(&text),
                        _ => {
                            self.live = Some(LiveText {
                                session: session_id,
                                execution: execution_id,
                                text,
                            });
                        }
                    }
                }
                Ok(ServerFrame::Activity { .. }) => {}
                // This client fell behind the live stream: drop the partial
                // answer rather than presenting a gap as if it were the text.
                Ok(ServerFrame::ActivityReset { subscription_id })
                    if self.subscription == Some(subscription_id) =>
                {
                    self.live = None;
                }
                Ok(ServerFrame::ActivityReset { .. }) => {}
                Ok(ServerFrame::SubscriptionClosed { reason, .. }) => {
                    self.subscription = None;
                    self.live = None;
                    closed = Some(reason);
                }
                // Responses are routed to the request that asked for them and
                // never reach this channel.
                Ok(ServerFrame::Response(_)) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Err(ClientError::Closed),
            }
        }
        if changed {
            self.refresh()?;
            // A turn that just ended frees the session for the next queued
            // line; this is the only place the queue drains, so one dispatch
            // per committed event and never two overlapping turns.
            self.flush_queue();
        }
        match closed {
            Some(SubscriptionClosedReason::HostShutdown) => {
                self.set_status(Level::Error, "the Runtime Host is shutting down");
                self.quit = true;
            }
            // A slow consumer lost its live queue, not a committed event:
            // replay from the last applied sequence recovers the gap.
            Some(SubscriptionClosedReason::SlowConsumer) => {
                self.set_status(
                    Level::Info,
                    "this client fell behind; replaying committed events",
                );
                self.subscribe()?;
            }
            Some(SubscriptionClosedReason::ClientUnsubscribed) | None => {}
        }
        // An open roster follows its runs while the reader watches: a child
        // that finishes shows the answer it produced without the pane being
        // re-opened, and a dropped re-read keeps the rows already on screen.
        if let Some(pane) = self.subagents.as_mut() {
            pane.ticks = pane.ticks.wrapping_add(1);
        }
        if self
            .subagents
            .as_ref()
            .is_some_and(|pane| pane.ticks.is_multiple_of(SUBAGENT_POLL_EVERY))
        {
            self.poll_subagents();
        }
        Ok(())
    }

    /// Apply one key. Every branch either edits local text or sends a request.
    pub(crate) fn handle_key(&mut self, key: Key) {
        // The terminal pane owns every key while it is open: a shell has a line
        // editor of its own, and a client that interpreted the keystroke first
        // could not drive vim or a pager. Ctrl-] is the one chord it reserves,
        // because the shell would otherwise receive it as an escape byte.
        if self.shell.is_some() {
            if key == Key::Ctrl(']') {
                self.close_terminal();
                return;
            }
            if let Some(bytes) = shell::encode(&key)
                && let Some(shell) = self.shell.as_mut()
            {
                shell.write(&bytes);
            }
            return;
        }
        // A choice list is modal: while it is up it owns every printable key as
        // filter text and the arrows as its highlight, because a reader
        // narrowing a list of models is not typing a turn. The chords it does
        // not claim - Ctrl-C to stop the running turn, Ctrl-D to quit - fall
        // through, so a turn that needs stopping is never held hostage by a
        // list.
        if self.picker_key(&key) {
            return;
        }
        // The trajectory pane owns the scrolling keys while it is open, the way
        // the other documents do: Escape returns to the transcript and the
        // page keys walk the log. Its offset is measured from the newest row,
        // so Up scrolls back in time and Down returns toward the tail.
        if self.trajectory.is_some() {
            match &key {
                Key::Escape => {
                    self.close_trajectory();
                    return;
                }
                Key::Up | Key::ScrollUp => {
                    self.scroll_trajectory(SCROLL as isize);
                    return;
                }
                Key::Down | Key::ScrollDown => {
                    self.scroll_trajectory(-(SCROLL as isize));
                    return;
                }
                Key::PageUp => {
                    self.scroll_trajectory(PAGE as isize);
                    return;
                }
                Key::PageDown => {
                    self.scroll_trajectory(-(PAGE as isize));
                    return;
                }
                _ => {}
            }
        }
        // The settings pane owns the scrolling keys while it is open, the way
        // the board does: the rail is a document the reader asked for, so
        // Escape returns to the transcript and the page keys walk its rows.
        // Typing still reaches the composer, so `/settings model` re-reads the
        // Host's resolution without closing the pane first.
        if self.settings.is_some() {
            match &key {
                Key::Escape => {
                    self.close_settings();
                    return;
                }
                Key::Up | Key::ScrollUp => {
                    self.scroll_settings(-(SCROLL as isize));
                    return;
                }
                Key::Down | Key::ScrollDown => {
                    self.scroll_settings(SCROLL as isize);
                    return;
                }
                Key::PageUp => {
                    self.scroll_settings(-(PAGE as isize));
                    return;
                }
                Key::PageDown => {
                    self.scroll_settings(PAGE as isize);
                    return;
                }
                _ => {}
            }
        }
        // The tasks pane owns the scrolling keys while it is open, the way the
        // memory pane does: the board is a document the reader asked for, so
        // Escape returns to the transcript and the page keys walk its rows.
        // Typing still reaches the composer, so `/tasks` re-reads the store
        // without closing the pane first.
        if self.tasks.is_some() {
            match &key {
                Key::Escape => {
                    self.close_tasks();
                    return;
                }
                Key::Up | Key::ScrollUp => {
                    self.scroll_tasks(-(SCROLL as isize));
                    return;
                }
                Key::Down | Key::ScrollDown => {
                    self.scroll_tasks(SCROLL as isize);
                    return;
                }
                Key::PageUp => {
                    self.scroll_tasks(-(PAGE as isize));
                    return;
                }
                Key::PageDown => {
                    self.scroll_tasks(PAGE as isize);
                    return;
                }
                _ => {}
            }
        }
        // The federation pane owns the scrolling keys while it is open, the way
        // the board does: Escape returns to the transcript and the page keys
        // walk the roster. Typing still reaches the composer, so `/federation`
        // re-reads the store without closing the pane first.
        if self.federation.is_some() {
            match &key {
                Key::Escape => {
                    self.close_federation();
                    return;
                }
                Key::Up | Key::ScrollUp => {
                    self.scroll_federation(-(SCROLL as isize));
                    return;
                }
                Key::Down | Key::ScrollDown => {
                    self.scroll_federation(SCROLL as isize);
                    return;
                }
                Key::PageUp => {
                    self.scroll_federation(-(PAGE as isize));
                    return;
                }
                Key::PageDown => {
                    self.scroll_federation(PAGE as isize);
                    return;
                }
                _ => {}
            }
        }
        // The browser pane owns the scrolling keys while it is open, the way
        // the federation pane does: Escape returns to the transcript and the
        // page keys walk the page. Typing still reaches the composer, so
        // `/browser` re-reads the Host without closing the pane first.
        if self.browser.is_some() {
            match &key {
                Key::Escape => {
                    self.close_browser();
                    return;
                }
                Key::Up | Key::ScrollUp => {
                    self.scroll_browser(-(SCROLL as isize));
                    return;
                }
                Key::Down | Key::ScrollDown => {
                    self.scroll_browser(SCROLL as isize);
                    return;
                }
                Key::PageUp => {
                    self.scroll_browser(-(PAGE as isize));
                    return;
                }
                Key::PageDown => {
                    self.scroll_browser(PAGE as isize);
                    return;
                }
                _ => {}
            }
        }
        // The files pane owns the navigating keys while it is open: Up and Down
        // walk the listing, Enter opens the highlighted row and Escape closes
        // the pane instead of discarding the draft the reader came back to.
        // Typing still reaches the composer, so `/files src` opens another
        // directory without closing the pane first.
        if self.files.is_some() {
            match &key {
                Key::Escape => {
                    self.close_files();
                    return;
                }
                Key::Up | Key::ScrollUp => {
                    self.move_files(-(SCROLL as isize));
                    return;
                }
                Key::Down | Key::ScrollDown => {
                    self.move_files(SCROLL as isize);
                    return;
                }
                Key::Enter => {
                    self.open_files_selection();
                    return;
                }
                Key::PageUp => {
                    self.move_files(-(PAGE as isize));
                    return;
                }
                Key::PageDown => {
                    self.move_files(PAGE as isize);
                    return;
                }
                _ => {}
            }
        }
        // The diff pane owns the same keys while it is open: Up and Down walk
        // the change rows, Enter opens the highlighted row's comparison and
        // Escape returns to the transcript. A patch has no rows to walk, so
        // there Up and Down scroll it instead - the pane decides which, the
        // way the files pane decides between a listing and a document.
        if self.diff.is_some() {
            match &key {
                Key::Escape => {
                    self.close_diff();
                    return;
                }
                Key::Up | Key::ScrollUp => {
                    self.move_diff(-(SCROLL as isize));
                    return;
                }
                Key::Down | Key::ScrollDown => {
                    self.move_diff(SCROLL as isize);
                    return;
                }
                Key::Enter => {
                    self.open_diff_selection();
                    return;
                }
                Key::PageUp => {
                    self.move_diff(-(PAGE as isize));
                    return;
                }
                Key::PageDown => {
                    self.move_diff(PAGE as isize);
                    return;
                }
                _ => {}
            }
        }
        // The extensions pane owns the scrolling keys while it is open, like the
        // memory pane: Escape returns to the transcript, Up/Down walk the rows,
        // and typing still reaches the composer so `/extensions <text>`
        // narrows the roster without closing it first.
        if self.extensions.is_some() {
            match &key {
                Key::Escape => {
                    self.close_extensions();
                    return;
                }
                Key::Up | Key::ScrollUp => {
                    self.scroll_extensions(-(SCROLL as isize));
                    return;
                }
                Key::Down | Key::ScrollDown => {
                    self.scroll_extensions(SCROLL as isize);
                    return;
                }
                Key::PageUp => {
                    self.scroll_extensions(-(PAGE as isize));
                    return;
                }
                Key::PageDown => {
                    self.scroll_extensions(PAGE as isize);
                    return;
                }
                _ => {}
            }
        }
        // The review pane owns the navigating keys while it is open, like the
        // files pane: Up/Down walk the change list, Enter opens the highlighted
        // change's diff, and a diff is a document that only scrolls. Escape
        // steps back to the list before it closes the pane, which is the
        // desktop's breadcrumb out of a diff.
        if self.review.is_some() {
            match &key {
                Key::Escape => {
                    self.back_review();
                    return;
                }
                Key::Up | Key::ScrollUp => {
                    self.move_review(-(SCROLL as isize));
                    return;
                }
                Key::Down | Key::ScrollDown => {
                    self.move_review(SCROLL as isize);
                    return;
                }
                Key::Enter => {
                    self.open_review_selection();
                    return;
                }
                Key::PageUp => {
                    self.move_review(-(PAGE as isize));
                    return;
                }
                Key::PageDown => {
                    self.move_review(PAGE as isize);
                    return;
                }
                _ => {}
            }
        }
        // The memory pane owns the scrolling keys while it is open, the way the
        // detail pane does: Escape returns to the transcript and Up/Down walk
        // the rows. Typing still reaches the composer, so `/memory <text>`
        // refines the search without closing the pane first.
        if self.memory.is_some() {
            match &key {
                Key::Escape => {
                    self.close_memory();
                    return;
                }
                Key::Up | Key::ScrollUp => {
                    self.scroll_memory(-(SCROLL as isize));
                    return;
                }
                Key::Down | Key::ScrollDown => {
                    self.scroll_memory(SCROLL as isize);
                    return;
                }
                Key::PageUp => {
                    self.scroll_memory(-(PAGE as isize));
                    return;
                }
                Key::PageDown => {
                    self.scroll_memory(PAGE as isize);
                    return;
                }
                _ => {}
            }
        }
        // The child-run pane walks its roster with the arrow keys and opens one
        // run with Enter, the way the review pane walks its changed files.
        // Escape unwinds one document at a time - the open run first, then the
        // roster - so reading an answer never throws the list away.
        if self.subagents.is_some() {
            match &key {
                Key::Escape => {
                    self.close_subagents();
                    return;
                }
                Key::Up | Key::ScrollUp => {
                    self.move_subagents(-(SCROLL as isize));
                    return;
                }
                Key::Down | Key::ScrollDown => {
                    self.move_subagents(SCROLL as isize);
                    return;
                }
                Key::Enter => {
                    self.open_subagents_selection();
                    return;
                }
                Key::PageUp => {
                    self.move_subagents(-(PAGE as isize));
                    return;
                }
                Key::PageDown => {
                    self.move_subagents(PAGE as isize);
                    return;
                }
                _ => {}
            }
        }
        // The audit pane owns the same keys: Escape returns to the transcript,
        // Up/Down walk the rows, and typing still reaches the composer so
        // `/audit <text>` narrows the list without closing it first.
        if self.audit.is_some() {
            match &key {
                Key::Escape => {
                    self.close_audit();
                    return;
                }
                Key::Up | Key::ScrollUp => {
                    self.scroll_audit(-(SCROLL as isize));
                    return;
                }
                Key::Down | Key::ScrollDown => {
                    self.scroll_audit(SCROLL as isize);
                    return;
                }
                Key::PageUp => {
                    self.scroll_audit(-(PAGE as isize));
                    return;
                }
                Key::PageDown => {
                    self.scroll_audit(PAGE as isize);
                    return;
                }
                _ => {}
            }
        }
        // An open detail pane owns the scrolling keys: the reader is inside a
        // tool call, so Up and Down walk its rows rather than the prompt
        // history, and Escape closes the pane instead of discarding the draft
        // the reader came back to. Typing still reaches the composer.
        if self.detail.is_some() {
            match &key {
                Key::Escape => {
                    self.close_detail();
                    return;
                }
                Key::Up | Key::ScrollUp => {
                    self.detail_scroll = self.detail_scroll.saturating_sub(SCROLL);
                    return;
                }
                Key::Down | Key::ScrollDown => {
                    self.detail_scroll = self.detail_scroll.saturating_add(SCROLL);
                    return;
                }
                Key::PageUp => {
                    self.detail_scroll = self.detail_scroll.saturating_sub(PAGE);
                    return;
                }
                Key::PageDown => {
                    self.detail_scroll = self.detail_scroll.saturating_add(PAGE);
                    return;
                }
                _ => {}
            }
        }
        // The completion list, while it is open, owns the navigating keys: Up
        // and Down walk its rows instead of the prompt history, Enter and Tab
        // accept the highlighted row, and Escape closes the list before it
        // reaches the composer.
        if self.menu.open() {
            match &key {
                Key::Up => {
                    self.menu.move_by(-1);
                    return;
                }
                Key::Down => {
                    self.menu.move_by(1);
                    return;
                }
                Key::Enter | Key::Tab => {
                    self.accept();
                    return;
                }
                Key::Escape => {
                    self.menu.close();
                    return;
                }
                _ => {}
            }
        }
        // The question card owns the walking keys while the prompt is empty:
        // Up/Down step through its options instead of the prompt history, and
        // Enter answers the highlighted one. The moment the reader types, the
        // composer takes every key back - which is the reader answering in
        // their own words - so the card never swallows a keystroke they meant
        // to type.
        if self.question_key(&key) {
            return;
        }
        match &key {
            Key::Char(character) => {
                self.input.insert(self.cursor, *character);
                self.cursor += character.len_utf8();
                self.sync_menu();
            }
            Key::Enter => self.submit(),
            Key::Newline => {
                self.newline();
                self.sync_menu();
            }
            // Ctrl-J is the terminal's other line break: some terminals never
            // deliver Shift-Enter, and opencode binds the same chord.
            Key::Ctrl('j') => {
                self.newline();
                self.sync_menu();
            }
            Key::Backspace => {
                self.backspace();
                self.sync_menu();
            }
            Key::Escape => {
                self.input.clear();
                self.cursor = 0;
                self.menu.close();
            }
            Key::Left => {
                if let Some((index, _)) = self.input[..self.cursor].char_indices().next_back() {
                    self.cursor = index;
                }
                self.sync_menu();
            }
            Key::Right => {
                if let Some(character) = self.input[self.cursor..].chars().next() {
                    self.cursor += character.len_utf8();
                }
                self.sync_menu();
            }
            // The line and word steps a terminal reader already has in their
            // fingers: Home/End and Ctrl-A/Ctrl-E for the line, the modifier
            // arrows for a word.
            Key::Home | Key::Ctrl('a') => {
                self.cursor = line_start(&self.input, self.cursor);
                self.sync_menu();
            }
            Key::End | Key::Ctrl('e') => {
                self.cursor = line_end(&self.input, self.cursor);
                self.sync_menu();
            }
            Key::WordLeft => {
                self.cursor = word_start(&self.input, self.cursor);
                self.sync_menu();
            }
            Key::WordRight => {
                self.cursor = word_end(&self.input, self.cursor);
                self.sync_menu();
            }
            Key::Delete => {
                delete_forward(&mut self.input, &mut self.cursor);
                self.sync_menu();
            }
            // The three kills are readline's own cuts, so a draft can be taken
            // apart without holding Backspace down.
            Key::Ctrl('k') => {
                kill_to_line_end(&mut self.input, &mut self.cursor);
                self.sync_menu();
            }
            Key::Ctrl('u') => {
                kill_to_line_start(&mut self.input, &mut self.cursor);
                self.sync_menu();
            }
            Key::Ctrl('w') => {
                kill_word(&mut self.input, &mut self.cursor);
                self.sync_menu();
            }
            Key::Up => {
                self.recall(true);
                self.sync_menu();
            }
            Key::Down => {
                self.recall(false);
                self.sync_menu();
            }
            Key::ScrollUp => self.scroll_transcript(SCROLL as isize),
            Key::ScrollDown => self.scroll_transcript(-(SCROLL as isize)),
            Key::PageUp => self.scroll_transcript(PAGE as isize),
            Key::PageDown => self.scroll_transcript(-(PAGE as isize)),
            Key::Tab => self.toggle_plan(),
            Key::Ctrl('n') => self.cycle(1),
            Key::Ctrl('p') => self.cycle(-1),
            Key::Ctrl('t') => self.create_session(None),
            Key::Ctrl('c') => self.cancel_execution(),
            Key::Ctrl('y') => self.decide_approval(true, None),
            Key::Ctrl('r') => self.decide_approval(false, None),
            Key::Ctrl('d') => self.quit = true,
            Key::Ctrl('l') => self.refresh_all(),
            // The chord every shell has carried for this gesture: hand the
            // prompt to the editor the reader already types in.
            Key::Ctrl('x') => self.edit_composer(),
            // A paste lands in the composer whole: its newlines are text, not
            // submits, so a multi-line block arrives as one edit.
            Key::Paste(text) => {
                self.input.insert_str(self.cursor, text);
                self.cursor += text.len();
                self.sync_menu();
            }
            // A click is the session rail's gesture: the pointer picks the
            // row the rail painted, and the frame's own layout is what maps
            // those cells back to a session.
            Key::Click { row, column } => self.click(*row, *column),
            Key::Ctrl(_) => {}
        }
    }

    /// The frame this client paints, plus where the terminal cursor belongs.
    ///
    /// Rows leave here painted rather than plain: [`Painter`] decides what each
    /// row is *about*, so a tone is never guessed at a call site and a reader
    /// whose terminal takes no colour keeps every shape the tone implies.
    pub(crate) fn render(
        &self,
        width: usize,
        height: usize,
    ) -> (Vec<Line<'static>>, Option<(usize, usize)>) {
        let width = width.max(40);
        let height = height.max(8);
        // Remember the cells this frame is about to fill: a click names a
        // screen row, and only this layout knows which session is painted
        // there.
        self.frame.set((width, height));
        // The composer is measured before the body: a multi-line prompt takes
        // rows from the transcript instead of pushing the frame past the
        // terminal and scrolling the header away.
        let (input_rows, cursor) = self.input_block(width);
        let menu_rows = self.menu_lines(width);
        // The choice list is measured with the popup: it sits where a list
        // attached to the composer sits, so its rows come out of the body's.
        let picker_rows = self.picker_lines(width);
        // The plan block is measured with the composer and the popup: it sits
        // above them, so its rows come out of the transcript's budget.
        let plan_rows = self.plan_lines(width);
        // The queue dock sits directly above the composer, under the plan
        // block, so a line waiting for the running turn is visible where it
        // would be typed rather than hidden behind the transcript.
        let queue_rows = self.queue_lines(width);
        // The question card sits where the answer is typed, so its rows come
        // out of the transcript's budget like the plan block's.
        let question_rows = self.card_rows(width);
        let body_rows = self.body_rows(width, height);

        let mut chrome = Painter::new(self.skin);
        let mut lines: Vec<Line<'static>> = Vec::with_capacity(height);
        // The frame's name is the heading; everything after it in the same row
        // is the state the frame was painted from, so it recedes.
        let header = truncate(&self.header(), width);
        lines.push(chrome.marker(&header, "bsh tui ", Tone::Heading, Tone::Muted));
        lines.push(chrome.tone(&rule(width), Tone::Muted));

        let sidebar = rail_width(width);
        let right_width = transcript_width(width);
        let left_lines = self.session_lines(sidebar.unwrap_or(width), body_rows);
        let (right_lines, _) = self.transcript(right_width, body_rows);
        // Each column is its own document, so each gets its own painter: the
        // tone a transcript row hands to its wrapped continuation must not leak
        // into the session list beside it.
        let mut left = Painter::new(self.skin);
        let mut right = Painter::new(self.skin);
        let top = self.session_top(body_rows);
        // The row a `/find` revealed is painted as a bar while it is still on
        // screen: a jump whose landing the reader cannot see is not a reveal.
        let focus_row = self
            .focus
            .zip(self.body_start.get())
            .and_then(|(focus, start)| focus.checked_sub(start))
            .filter(|row| *row < body_rows);
        for row in 0..body_rows {
            // Only the transcript column takes the bar: the rail beside it still
            // means "the open session".
            let right_row = match focus_row {
                Some(focus) if focus == row => {
                    right.tone(&pad(&right_lines[row], right_width), Tone::Select)
                }
                _ => right.row(&right_lines[row]),
            };
            let line = match sidebar {
                Some(left_width) => {
                    // The list's pointer is the one marker the frame does not
                    // classify on its own: it means "the open session" on one
                    // screen and "the highlighted row" on another.
                    let text = pad(&left_lines[row], left_width);
                    // A row past the end of the list names no session, so it
                    // cannot be the selection either: a reversed blank bar
                    // would read as a session that is not there.
                    let named = top + row < self.sessions.len();
                    let column = if named && top + row == self.selected {
                        left.marker(&text, "› ", Tone::Select, Tone::Select)
                    } else {
                        left.tone(&text, Tone::Body)
                    };
                    let mut spans = column.spans;
                    spans.push(Span::styled("│", self.skin.style(Tone::Muted)));
                    spans.extend(right_row.spans);
                    Line::from(spans)
                }
                None => right_row,
            };
            lines.push(line);
        }
        // Both column builders always return `body_rows` entries, so the frame
        // keeps a fixed height while events arrive.
        lines.push(chrome.tone(&rule(width), Tone::Muted));
        // The desktop draws its to-do panel above the composer; the terminal
        // draws the same durable plan in the same place, so a reader watching
        // a turn sees progress without scrolling back through tool calls.
        for row in &plan_rows {
            lines.push(chrome.row(row));
        }
        for row in &queue_rows {
            lines.push(chrome.row(row));
        }
        // The desktop draws its question card inside the transcript; a terminal
        // answers with the next line of the prompt, so the card is drawn where
        // that line is about to be typed.
        for row in &question_rows {
            lines.push(chrome.row(row));
        }
        // The row above the composer is a verdict when something is waiting and
        // metadata otherwise: an approval is a warning, a refusal is an error,
        // and a note the reader asked for is dim.
        let prompt = self.prompt_line(width);
        let tone = if self.pending_approval().is_some() {
            Tone::Warn
        } else {
            match self.status.as_ref().map(|(level, _)| *level) {
                Some(Level::Error) => Tone::Error,
                Some(Level::Info) | None => Tone::Muted,
            }
        };
        lines.push(if prompt.is_empty() {
            chrome.blank()
        } else {
            chrome.tone(&prompt, tone)
        });
        // The popup sits between the status row and the composer, so it reads
        // as an attachment to what is being typed rather than as transcript.
        // A choice list stands in the same place, one row per choice.
        //
        // The list's top row is recorded for the pointer: a click arrives as a
        // screen row, and this is the only record of which choice stood there.
        self.picker_at
            .set((!picker_rows.is_empty()).then_some(lines.len()));
        for row in &picker_rows {
            let tone = if row.starts_with('›') {
                Tone::Select
            } else {
                Tone::Body
            };
            lines.push(chrome.tone(row, tone));
        }
        // The popup's rows start immediately, so the row its first candidate
        // stands on is the whole of what a click needs to name a candidate.
        self.menu_at
            .set((!menu_rows.is_empty()).then_some(lines.len()));
        for row in &menu_rows {
            let tone = if row.starts_with('›') {
                Tone::Select
            } else {
                Tone::Body
            };
            lines.push(chrome.tone(row, tone));
        }
        let input_row = lines.len();
        for row in &input_rows {
            // The prompt's first row is introduced by the composer's marker; a
            // wrapped or scrolled row is prose and nothing else.
            if row.starts_with("› ") {
                lines.push(chrome.marker(row, "› ", Tone::Accent, Tone::Body));
            } else {
                lines.push(chrome.tone(row, Tone::Body));
            }
        }
        // The footer spends its one row on the keys this state needs: a reader
        // mid-approval is not told about the shell, and a reader scrolled into
        // a pane is told how to leave it.
        let footer = skin::footer_text(self.footer(), width);
        lines.push(chrome.tone(&footer, Tone::Muted));
        let column = cursor.1.min(width.saturating_sub(1));
        (lines, Some((input_row + cursor.0, column)))
    }

    /// The terminal pane's frame, when a shell is open.
    ///
    /// The ordinary renderer answers with plain strings: every other document
    /// in this client is text. A shell's grid carries a colour per cell, so
    /// the pane returns styled lines and the cursor cell the frame should
    /// place - the shell, not this client, decides where the cursor is.
    pub(crate) fn shell_lines(&mut self, width: usize, height: usize) -> Option<ShellFrame> {
        let width = width.max(1);
        let height = height.max(2);
        // One row for the rule and one for the hint; the grid takes the rest.
        let grid_rows = height - 2;
        let shell = self.shell.as_mut()?;
        shell.resize(grid_rows as u16, width as u16);
        let cells = shell.cells();
        let mut lines = Vec::with_capacity(height);
        let mut cursor = None;
        for (row, cells) in cells.iter().enumerate().take(grid_rows) {
            // Runs of one style become one span: a screen is mostly its
            // default colours, and a span per cell would hand ratatui a
            // hundred widgets per row on every frame.
            let mut spans = Vec::new();
            let mut run: Option<(Style, String)> = None;
            for (column, cell) in cells.iter().enumerate() {
                // The shell's program sits on this cell; the frame puts the
                // terminal's own cursor there instead of guessing.
                if cell.cursor {
                    cursor = Some((row, column.min(width.saturating_sub(1))));
                }
                let style = Style::default().fg(rgb(cell.fg)).bg(rgb(cell.bg));
                // A blank cell is a space, not nothing: the grid's columns
                // have to line up or a prompt shifts under the cursor.
                let text = if cell.text.is_empty() {
                    " ".to_string()
                } else {
                    cell.text.clone()
                };
                match &mut run {
                    Some((current, buffer)) if *current == style => buffer.push_str(&text),
                    _ => {
                        if let Some((current, buffer)) = run.take() {
                            spans.push(Span::styled(buffer, current));
                        }
                        run = Some((style, text));
                    }
                }
            }
            if let Some((current, buffer)) = run {
                spans.push(Span::styled(buffer, current));
            }
            lines.push(Line::from(spans));
        }
        // A grid shorter than the frame (a shell that resized itself down)
        // still keeps the hint on the frame's last row.
        while lines.len() < grid_rows {
            lines.push(Line::from(String::new()));
        }
        // The hint row is the shell's own chrome, painted from the same skin as
        // the rest of the client so the pane does not switch vocabulary.
        let mut painter = Painter::new(self.skin);
        lines.push(painter.tone(&rule(width), Tone::Muted));
        lines.push(painter.tone(&truncate(&shell.hint(), width), Tone::Muted));
        Some((lines, cursor))
    }

    /// Open the client's shell in place of the transcript.
    ///
    /// The desktop offers the same pane as a tab over its right panel. A
    /// terminal has no Ghostty surface, so this runs the software terminal
    /// every platform shares and starts the shell at the workspace root the
    /// Host serves. One shell per client: asking again reports the live one
    /// rather than orphaning a second.
    fn terminal_command(&mut self) {
        if let Some(shell) = self.shell.as_mut() {
            let hint = shell.hint();
            self.set_status(Level::Info, hint);
            return;
        }
        match Shell::spawn(24, 80, self.workspace.as_deref()) {
            Ok(mut shell) => {
                let hint = shell.hint();
                self.shell = Some(shell);
                // The shell paints the whole frame, so a pane left behind it
                // would come back looking stale after Ctrl-].
                self.settings = None;
                self.set_status(Level::Info, hint);
            }
            Err(error) => self.set_status(Level::Error, format!("terminal: {error}")),
        }
    }

    /// Close the shell and give the frame back to the transcript.
    fn close_terminal(&mut self) {
        if self.shell.take().is_some() {
            self.set_status(Level::Info, "terminal closed");
        }
    }

    /// Read what the Host's settings resolve to and open the Settings pane.
    ///
    /// The query is asked on every open rather than cached from attach: the
    /// desktop can save a model, a permission or a vault entry while this
    /// terminal is attached, and a reader who opened the pane asked what a turn
    /// runs with now. A section narrows the rail to one cell; an unknown name
    /// still opens the pane, because the Host answers with the names it has and
    /// that answer belongs in front of the reader rather than in one status line
    /// that scrolls the transcript away.
    fn settings_command(&mut self, argument: Option<&str>) {
        // The Host compares section ids case-insensitively, so the name is
        // folded here too: otherwise `Model` would ask for the right cell and
        // then filter the answer out by its own spelling.
        let section = argument
            .map(str::trim)
            .filter(|argument| !argument.is_empty())
            .map(str::to_ascii_lowercase);
        match self.client.settings(section.as_deref()) {
            Ok(report) => {
                let summary = settings_summary(&report, section.as_deref());
                self.settings = Some(SettingsPane {
                    report,
                    section: section.clone(),
                    scroll: 0,
                });
                // One pane at a time, like `/tasks` and `/memory`: the reader
                // asked for the settings, so that is what the body shows.
                self.memory = None;
                self.files = None;
                self.diff = None;
                self.tasks = None;
                self.audit = None;
                self.extensions = None;
                self.review = None;
                self.federation = None;
                self.browser = None;
                self.trajectory = None;
                self.detail = None;
                let level = if summary.starts_with("no settings section") {
                    Level::Error
                } else {
                    Level::Info
                };
                self.set_status(level, summary);
            }
            Err(error) => self.client_error(error),
        }
    }

    /// Leave the Settings pane and keep whatever the composer holds.
    fn close_settings(&mut self) {
        self.settings = None;
        self.set_status(Level::Info, "settings closed");
    }

    /// Write one settings field through the Host, or open the rail that lists
    /// the writable ones.
    ///
    /// The Host owns what a value may be: this client sends the key and the
    /// text exactly as it was typed and reports whatever comes back, so a
    /// spelling the desktop's form refuses is refused here too, with the same
    /// reason. On success the pane is replaced by the report the Host
    /// returned, which is the resolution the NEXT turn will use, instead of a
    /// guess about what was stored.
    fn set_command(&mut self, key: Option<&str>, value: Option<&str>) {
        let Some(key) = key else {
            // No argument is the discovery path: open the rail, then say how
            // many rows a write can address instead of making the reader count
            // the stars in the body.
            self.settings_command(None);
            if let Some(pane) = self.settings.as_ref() {
                let editable = editable_rows(&pane.report).len();
                // Which fields a bare `/set <key>` can clear is a fact the
                // writer owns, so the hint is read from that table instead of
                // from a second list kept here that would drift from it.
                let clearable = editable_rows(&pane.report)
                    .into_iter()
                    .filter_map(|row| row.key.as_deref())
                    .filter(|key| crate::settings::edit::accepts_empty(key))
                    .collect::<Vec<_>>()
                    .join(", ");
                let summary = if clearable.is_empty() {
                    format!("{editable} writable row(s), marked * — /set <key> <value> writes")
                } else {
                    format!(
                        "{editable} writable row(s), marked * — /set <key> <value> writes; a bare /set clears {clearable}"
                    )
                };
                self.set_status(Level::Info, summary);
            }
            return;
        };
        let edit = SettingEdit {
            key: key.to_string(),
            value: value.unwrap_or_default().to_string(),
        };
        match self.client.update_settings(vec![edit]) {
            Ok(report) => {
                // Keep the section the reader narrowed to, and the scroll: a
                // write refreshes values in place instead of throwing the
                // reader back to the top of a rail they were reading.
                if let Some(pane) = self.settings.as_ref() {
                    let section = pane.section.clone();
                    let scroll = pane.scroll;
                    self.settings = Some(SettingsPane {
                        report: report.clone(),
                        section,
                        scroll,
                    });
                }
                self.set_status(Level::Info, setting_summary(&report, key));
            }
            Err(error) => self.client_error(error),
        }
    }

    /// Walk the Settings pane's rows by one step.
    fn scroll_settings(&mut self, delta: isize) {
        let Some(pane) = self.settings.as_mut() else {
            return;
        };
        pane.scroll = pane.scroll.saturating_add_signed(delta);
    }

    /// Follow the composer into the completion list.
    fn sync_menu(&mut self) {
        let root = self.workspace.as_deref();
        // The skills the Host advertises are the rows a `/` query ranks, so the
        // terminal offers the same catalogue the desktop palette does without
        // keeping a second copy of the build's skill resolution.
        // Borrowed rather than cloned: a 120-skill catalogue must not copy its
        // names on every keystroke, and the two fields are disjoint.
        //
        // The `@` card's session block is the same list the sidebar draws: an
        // open chat from the picker is a conversation a reader can already
        // reach with Ctrl-N, so the rows are read from the snapshot rather than
        // queried again on every keystroke.
        let workspace = workspace_label(root);
        let sessions: Vec<menu::SessionChoice> = self
            .sessions
            .iter()
            .map(|session| menu::SessionChoice {
                id: session.session_id.to_string(),
                title: session.title.clone(),
                workspace: workspace.clone(),
            })
            .collect();
        self.menu.sync(
            &self.input,
            self.cursor,
            root,
            &self.catalogue.skills,
            &sessions,
        );
    }

    /// Accept the highlighted completion row.
    ///
    /// A bare command's row is the line the reader would have typed, so it
    /// runs now; every other row leaves completed text in the composer. A
    /// session row is the desktop card's third gesture: it opens the chat it
    /// names and sends nothing.
    fn accept(&mut self) {
        match self.menu.accept(&mut self.input, &mut self.cursor) {
            Some(menu::Accepted::Insert(true)) => self.submit(),
            Some(menu::Accepted::Session(id)) => self.open_session(&id),
            Some(menu::Accepted::Insert(false)) | None => {}
        }
        self.sync_menu();
    }

    /// The completion popup's rows, or none when it is closed.
    fn menu_lines(&self, width: usize) -> Vec<String> {
        if !self.menu.open() {
            return Vec::new();
        }
        let rows = self.menu.rows();
        // The window is the menu's own, so the frame and a click read the same
        // arithmetic: a pointer names a screen row, and the row it names is
        // only a candidate because this window put it there.
        let (start, visible) = self.menu.window();
        let marker = if self.menu.truncated() { "…" } else { " " };
        rows.iter()
            .skip(start)
            .take(visible)
            .enumerate()
            .map(|(offset, row)| {
                let active = start + offset == self.menu.active();
                let pointer = if active { "›" } else { marker };
                let detail = if row.detail.is_empty() {
                    String::new()
                } else {
                    format!("  {}", row.detail)
                };
                truncate(&format!("{pointer} {}{detail}", row.label), width)
            })
            .collect()
    }

    /// Open the list a choice-taking command names, or say why there is none.
    ///
    /// The rows come from the catalogue this client already holds, so opening a
    /// list costs no query and no settings read. A command with nothing to
    /// offer leaves the reader the sentence it always gave instead of an empty
    /// box: a marker with no choice under it asks the reader to walk rows that
    /// are not there.
    fn open_picker(&mut self, kind: picker::Kind) {
        // `/sessions`, `/trash` and `/sessions archived` list this workspace,
        // not the catalogue: their rows come from the snapshot the printed
        // listing reads, so the list and the numbers agree.
        let mut items = match kind {
            picker::Kind::Session => {
                session_items(&self.sessions, Some(self.selected), picker::Kind::Session)
            }
            picker::Kind::Trash => session_items(&self.trash, None, picker::Kind::Trash),
            picker::Kind::Archived => session_items(&self.archived, None, picker::Kind::Archived),
            _ => kind.items(&self.catalogue),
        };
        if items.is_empty() {
            self.picker = None;
            self.set_status(Level::Error, kind.empty());
            return;
        }
        // The row the session already runs is ticked, the way the desktop
        // ticks the active model: the list then answers "what do I have" and
        // "what might I pick" at once. A client with no session open ticks
        // nothing rather than guessing at the machine default. A session list
        // is ticked by its own builder, against the chat that is open.
        if matches!(
            kind,
            picker::Kind::Session | picker::Kind::Trash | picker::Kind::Archived
        ) {
            self.picker = Some(picker::Picker::new(kind.title(), items));
            return;
        }
        if let Some(config) = self.session.as_ref().map(|session| &session.config) {
            kind.mark(&mut items, config);
        }
        self.picker = Some(picker::Picker::new(kind.title(), items));
    }

    /// Put the list away, committing nothing.
    fn close_picker(&mut self) {
        self.picker = None;
    }

    /// Commit the row under the highlight: the same command a typed name sends.
    ///
    /// A pick is not a second path to the Host: it becomes the command the
    /// parser would have produced from `/model provider model`, so a chosen
    /// pair and a typed one make the identical call.
    fn commit_picker(&mut self) {
        let Some(choice) = self.picker.as_ref().and_then(picker::Picker::choice) else {
            return;
        };
        self.picker = None;
        self.run_command(pick_command(choice));
    }

    /// The list's keys, while it is open.
    ///
    /// Returns whether the list consumed the key. Up and Down walk the rows, a
    /// page key walks a page, Enter commits and Escape puts it away without
    /// sending anything; every other chord reaches the client, which is what
    /// keeps Ctrl-C working while a list is up.
    fn picker_key(&mut self, key: &Key) -> bool {
        if self.picker.is_none() {
            return false;
        }
        let delta = match key {
            Key::Up | Key::ScrollUp => -1,
            Key::Down | Key::ScrollDown => 1,
            Key::PageUp => -(PAGE as isize),
            Key::PageDown => PAGE as isize,
            Key::Enter => {
                self.commit_picker();
                return true;
            }
            Key::Escape => {
                self.close_picker();
                return true;
            }
            Key::Backspace => {
                if let Some(picker) = self.picker.as_mut() {
                    picker.erase();
                }
                return true;
            }
            // A printable character narrows the list. A control chord is not
            // filter text: it falls through to the client below.
            Key::Char(character) if !character.is_control() => {
                if let Some(picker) = self.picker.as_mut() {
                    picker.typed(*character);
                }
                return true;
            }
            _ => return false,
        };
        if let Some(picker) = self.picker.as_mut() {
            picker.move_by(delta);
        }
        true
    }

    /// The list's rows for this frame, empty when it is closed.
    fn picker_lines(&self, width: usize) -> Vec<String> {
        self.picker
            .as_ref()
            .map(|picker| picker_lines(picker, width))
            .unwrap_or_default()
    }

    fn header(&self) -> String {
        let phase = self
            .host
            .as_ref()
            .map(|status| format!("{:?}", status.phase).to_lowercase())
            .unwrap_or_else(|| "unknown".to_string());
        let activity = self
            .host
            .as_ref()
            .map(|status| format!("{:?}", status.activity).to_lowercase())
            .unwrap_or_default();
        let title = match &self.session {
            Some(session) => session.title.clone(),
            None => "no session".to_string(),
        };
        // The desktop shows the model, preset, permission and plan chips above
        // the transcript; the terminal shows the same facts on one line.
        let config = self
            .session
            .as_ref()
            .map(|session| config_summary(&session.config))
            .unwrap_or_else(|| "no configuration".to_string());
        // The desktop's goal panel is one more fact above the transcript; the
        // terminal shows its state and round counter on the header line.
        let goal = self
            .session
            .as_ref()
            .and_then(|session| session.goal.as_ref())
            .map(|goal| {
                let state = format!("{:?}", goal.state).to_lowercase();
                format!("goal {state} {}/{}", goal.rounds, goal.max_rounds)
            })
            .unwrap_or_else(|| "no goal".to_string());
        // The desktop's stat pills under the composer total the session's
        // turns; the terminal states the same figure on the line that already
        // carries the model, permission and plan chips.
        let usage = self
            .session
            .as_ref()
            .and_then(session_usage)
            .map(|usage| usage_chip(&usage))
            .unwrap_or_else(|| "no usage".to_string());
        format!(
            "bsh tui · {title} · {config} · {goal} · {usage} · host {phase}/{activity} · {} session(s)",
            self.sessions.len()
        )
    }

    /// The row above the input: a pending approval, the last status, or blank.
    fn prompt_line(&self, width: usize) -> String {
        if let Some(approval) = self.pending_approval() {
            return approval_line(approval, width);
        }
        match &self.status {
            Some((level, message)) => {
                let prefix = match level {
                    Level::Info => "",
                    Level::Error => "error: ",
                };
                truncate(&format!("{prefix}{message}"), width)
            }
            None => String::new(),
        }
    }

    /// Which keys this frame's footer should offer, given what is on screen.
    ///
    /// The order is the order of obligation: an approval waits on a keystroke,
    /// a question card waits on an answer, an open popup or pane owns the next
    /// key, and only then does a running turn decide that a line would queue.
    fn footer(&self) -> Footer {
        if self.pending_approval().is_some() {
            Footer::Approval
        } else if self.question.is_some() {
            Footer::Question
        } else if self.picker.is_some() {
            Footer::Picker
        } else if self.menu.open() {
            Footer::Menu
        } else if self.pane_open() {
            Footer::Pane
        } else if self.busy() {
            Footer::Busy
        } else {
            Footer::Idle
        }
    }

    /// Whether a pane has taken the body. Every pane closes on Escape and
    /// scrolls on its own keys, so the footer says so once rather than once per
    /// pane.
    fn pane_open(&self) -> bool {
        self.detail.is_some()
            || self.memory.is_some()
            || self.files.is_some()
            || self.diff.is_some()
            || self.tasks.is_some()
            || self.audit.is_some()
            || self.extensions.is_some()
            || self.review.is_some()
            || self.federation.is_some()
            || self.subagents.is_some()
            || self.browser.is_some()
            || self.trajectory.is_some()
            || self.settings.is_some()
    }

    /// The plan block: the model's working plan above the composer.
    ///
    /// It is the terminal's form of the desktop's to-do panel, and it is
    /// built from the projection rather than from the tool calls this turn
    /// happened to make: the plan is durable session state, so a client that
    /// attached late still draws the steps the model last wrote. Absent while
    /// the session has no plan.
    fn plan_lines(&self, width: usize) -> Vec<String> {
        let Some(session) = self.session.as_ref() else {
            return Vec::new();
        };
        plan_block(&session.plan, width)
    }

    /// The queue dock: the lines the reader typed while a turn was running.
    ///
    /// The desktop keeps those lines in its queue dock above the composer
    /// instead of starting a second execution, and the terminal draws the same
    /// inbox in the same place. It is client state, not projection state: the
    /// Host journals the turns it accepted, so a line waiting here is text the
    /// reader has not sent yet.
    fn queue_lines(&self, width: usize) -> Vec<String> {
        queue_block(&self.queue, width)
    }

    /// The composer rows and the row and column the terminal cursor sits on.
    fn input_block(&self, width: usize) -> (Vec<String>, (usize, usize)) {
        composer(&self.input, self.cursor, width)
    }

    /// The rows the body gets once the composer and every dock around it have
    /// taken theirs.
    ///
    /// The frame and the pointer both measure with this: a click is tested
    /// against the same arithmetic that painted the row under it, even while
    /// a plan block, a queued line or a question card is up.
    fn body_rows(&self, width: usize, height: usize) -> usize {
        let (input_rows, _) = self.input_block(width);
        let menu_rows = self.menu_lines(width);
        let picker_rows = self.picker_lines(width);
        let plan_rows = self.plan_lines(width);
        let queue_rows = self.queue_lines(width);
        let question_rows = self.card_rows(width);
        height
            .saturating_sub(
                input_rows.len()
                    + menu_rows.len()
                    + picker_rows.len()
                    + plan_rows.len()
                    + queue_rows.len()
                    + question_rows.len()
                    + 5,
            )
            .max(1)
    }

    /// The first session row the sidebar shows for a body `rows` tall: the
    /// window keeps the selected row on its last line, the way a list does.
    fn session_top(&self, rows: usize) -> usize {
        self.selected.saturating_sub(rows.saturating_sub(1))
    }

    fn session_lines(&self, width: usize, rows: usize) -> Vec<String> {
        let start = self.session_top(rows);
        let mut lines: Vec<String> = self
            .sessions
            .iter()
            .skip(start)
            .take(rows)
            .enumerate()
            .map(|(offset, session)| {
                let index = start + offset;
                let marker = if index == self.selected { "› " } else { "  " };
                let state = match session.active_execution {
                    Some(ExecutionStatus::WaitingApproval) => "?",
                    Some(_) => "●",
                    None => " ",
                };
                let title_width = width.saturating_sub(5);
                format!("{marker}{} {state}", truncate(&session.title, title_width))
            })
            .collect();
        lines.resize(rows, String::new());
        lines
    }

    /// The transcript column: the rows it paints and, for each row, the tool
    /// call that row stands for.
    ///
    /// A pane is a document the reader asked to read, so its rows name
    /// nothing; a timeline row that draws a call does, because the desktop
    /// opens that call's detail when its row is clicked.
    fn transcript(&self, width: usize, rows: usize) -> (Vec<String>, Vec<Option<usize>>) {
        // A pane answers with the rows it drew and no targets, and it is not
        // the timeline: nothing on it is a document row `/find` could name.
        let quiet = |lines: Vec<String>| {
            self.body_start.set(None);
            self.document.borrow_mut().clear();
            (lines, vec![None; rows])
        };
        // The settings pane replaces the timeline while it is open: the reader
        // asked what a turn resolves from, so that is what the body shows.
        if let Some(pane) = self.settings.as_ref() {
            return quiet(settings_pane(pane, width, rows));
        }
        // The trajectory pane replaces the timeline while it is open: the
        // reader asked for the typed event log, so that is what the body shows.
        if let Some(pane) = self.trajectory.as_ref() {
            return quiet(trajectory_pane(pane, self.session.as_ref(), width, rows));
        }
        // The tasks pane replaces the timeline while it is open: the reader
        // asked for the workspace's board, so the board is what the body shows.
        if let Some(pane) = self.tasks.as_ref() {
            return quiet(tasks_pane(pane, width, rows));
        }
        // The federation pane replaces it the same way: the reader asked who
        // this node trusts, so the roster is what the body shows.
        if let Some(pane) = self.federation.as_ref() {
            return quiet(federation_pane(pane, width, rows));
        }
        // The child-run pane replaces it the same way: the reader asked what
        // this conversation delegated, so the roster is what the body shows.
        if let Some(pane) = self.subagents.as_ref() {
            return quiet(subagents_pane(pane, width, rows));
        }
        // The browser pane replaces it the same way: the reader asked what the
        // agent's browser read, so the page is what the body shows.
        if let Some(pane) = self.browser.as_ref() {
            return quiet(browser_pane(pane, width, rows));
        }
        // The files pane replaces the timeline while it is open, the way the
        // other panes do: the reader asked to browse the workspace, so the
        // workspace is what the body shows.
        if let Some(pane) = self.files.as_ref() {
            return quiet(files_pane(pane, width, rows));
        }
        // The diff pane replaces it the same way: the reader asked what the
        // working copy changed, so the comparison is what the body shows.
        if let Some(pane) = self.diff.as_ref() {
            return quiet(diff_pane(pane, width, rows));
        }
        // The memory pane replaces the timeline while it is open, like the
        // detail pane: the reader asked to read the store, so the store is what
        // the body shows.
        if let Some(pane) = self.audit.as_ref() {
            return quiet(audit_pane(pane, self.session.as_ref(), width, rows));
        }
        if let Some(pane) = self.extensions.as_ref() {
            return quiet(extensions_pane(pane, width, rows));
        }
        // The review pane replaces the timeline the same way: the reader asked
        // what a turn changed, so the changeset is what the body shows.
        if let Some(pane) = self.review.as_ref() {
            return quiet(review_pane(pane, width, rows));
        }
        if let Some(pane) = self.memory.as_ref() {
            return quiet(memory_pane(pane, width, rows));
        }
        // A reader who asked for one call sees that call instead of the
        // timeline: the pane carries its own title row, so the session, the
        // call's ordinal and the way back stay visible while it scrolls.
        if let Some(index) = self.detail {
            return quiet(detail_pane(
                self.session.as_ref(),
                index,
                width,
                rows,
                self.detail_scroll,
            ));
        }
        let mut lines: Vec<(String, Option<usize>)> = Vec::new();
        match &self.session {
            // No session is the same card with the other first fact: the
            // folder still names where a turn would run, and the key that
            // opens one is the answer to "why is nothing here?".
            None => lines.extend(targets(
                hero_rows(None, self.workspace.as_deref(), width),
                None,
            )),
            Some(session) => {
                // One timeline: a tool call belongs between the user turn that
                // caused it and the assistant text that follows, so the
                // transcript orders by committed time rather than by kind. Every
                // row of a call names the call it draws: the name, a failed
                // output or a changed patch row all open the same detail pane
                // the desktop opens.
                let mut entries: Vec<(Entry, Option<usize>)> = session
                    .messages
                    .iter()
                    .map(|message| (Entry::Message(message), None))
                    .chain(
                        session
                            .tool_calls
                            .iter()
                            .enumerate()
                            .map(|(index, call)| (Entry::Tool(call), Some(index))),
                    )
                    .collect();
                entries.sort_by_key(|(entry, _)| entry.at());
                for (entry, hit) in entries {
                    match entry {
                        Entry::Message(message) => {
                            let speaker = match (message.compaction, message.role) {
                                // A checkpoint is not something a participant
                                // said: it is the condensed history the Host
                                // kept, shown on its own row.
                                (true, _) => "checkpoint › ",
                                (false, MessageRole::User) => "you › ",
                                (false, MessageRole::Assistant) => "bsh › ",
                            };
                            lines
                                .extend(targets(message_rows(speaker, &message.text, width), None));
                        }
                        Entry::Tool(call) => lines.extend(targets(tool_lines(call, width), hit)),
                    }
                }
                for execution in &session.executions {
                    match execution.status {
                        ExecutionStatus::Failed
                        | ExecutionStatus::Interrupted
                        | ExecutionStatus::NeedsIntervention => {
                            let detail = execution.detail.as_deref().unwrap_or("no detail");
                            lines.extend(targets(
                                wrap(
                                    "! ",
                                    &format!(
                                        "{}: {detail}",
                                        format!("{:?}", execution.status).to_lowercase()
                                    ),
                                    width,
                                ),
                                None,
                            ));
                        }
                        ExecutionStatus::Queued | ExecutionStatus::Running => {
                            let live = self.live.as_ref().filter(|live| {
                                live.session == session.session_id
                                    && live.execution == execution.execution_id
                                    && !live.text.is_empty()
                            });
                            match live {
                                // Same claim the desktop makes while a turn runs:
                                // the streamed text is shown under the speaker
                                // prefix, then replaced by the committed answer.
                                Some(live) => lines.extend(targets(
                                    message_rows("bsh › ", &live.text, width),
                                    None,
                                )),
                                None => lines.extend(targets(
                                    wrap("… ", "the Host is running this turn", width),
                                    None,
                                )),
                            }
                        }
                        ExecutionStatus::WaitingApproval => {
                            lines.extend(targets(
                                wrap("… ", "waiting for an operator decision", width),
                                None,
                            ));
                        }
                        ExecutionStatus::Completed | ExecutionStatus::Cancelled => {}
                    }
                }
                for approval in &session.approvals {
                    if approval.status == ApprovalStatus::Pending {
                        lines.extend(targets(
                            wrap(
                                "? ",
                                &format!("approval pending: {}", approval.summary),
                                width,
                            ),
                            None,
                        ));
                    }
                }
                if lines.is_empty() {
                    lines.extend(targets(
                        hero_rows(Some(session), self.workspace.as_deref(), width),
                        None,
                    ));
                }
            }
        }
        // The document is kept for `/find`: the rows the reader searches are
        // exactly the rows this frame painted, so a hit names a row of the
        // screen rather than a row of some other wrapping.
        let start = window_bounds(lines.len(), rows, self.scroll).0;
        self.body_start.set(Some(start));
        let document = self.document.replace(lines);
        let window = window_rows(&document, rows, self.scroll);
        drop(document);
        window
    }

    fn current_session(&self) -> Option<SessionId> {
        self.sessions
            .get(self.selected)
            .map(|session| session.session_id)
    }

    fn pending_approval(&self) -> Option<&ApprovalView> {
        self.session.as_ref().and_then(|session| {
            session
                .approvals
                .iter()
                .find(|approval| approval.status == ApprovalStatus::Pending)
        })
    }

    fn select(&mut self, index: usize) {
        if self.sessions.is_empty() {
            return;
        }
        self.selected = index.min(self.sessions.len() - 1);
        self.scroll = 0;
        // A reveal numbers a row of the conversation that was open, so the bar
        // it drew means nothing in the next one.
        self.focus = None;
        // A call index belongs to the session that made it, so switching
        // sessions returns to the transcript instead of opening another
        // session's call under the same number. The question card is the same
        // kind of row: it belongs to the call that asked it.
        self.close_detail();
        self.question = None;
        self.question_dismissed = None;
        self.refresh_session();
    }

    /// Open the session a completion row names, the `@` card's own gesture.
    ///
    /// The picker ranks titles the client already holds, so the id is resolved
    /// against that snapshot: it names a row `/sessions` would also have
    /// offered, and a stale id is reported rather than silently opening the
    /// nearest chat.
    fn open_session(&mut self, id: &str) {
        let Some(index) = session_index(&self.sessions, id) else {
            self.set_status(
                Level::Error,
                "that session is no longer here; /sessions lists the current ones",
            );
            return;
        };
        self.select(index);
        let title = self
            .session
            .as_ref()
            .map(|session| session.title.clone())
            .unwrap_or_default();
        self.set_status(Level::Info, format!("opened session: {title}"));
    }

    fn cycle(&mut self, delta: i32) {
        if self.sessions.is_empty() {
            return;
        }
        let len = self.sessions.len();
        let next = (self.selected as i32 + delta).rem_euclid(len as i32) as usize;
        self.select(next);
    }

    /// Open the session a click on the rail names, or the call a click on a
    /// transcript row names.
    ///
    /// The pointer arrives in screen cells. The rail's rows address a session;
    /// the transcript's rows address a tool call, the way the desktop's do,
    /// while its prose and blank rows address nothing.
    fn click(&mut self, row: usize, column: usize) {
        // An open choice list owns the pointer while it is drawn: it sits over
        // the rows under it, and a click on its title is still its click.
        if self.click_picker(row) {
            return;
        }
        // The completion popup stands in the same place and takes its rows the
        // same way: a click on a candidate is the candidate Enter accepts.
        if self.click_menu(row) {
            return;
        }
        let (width, height) = self.frame.get();
        let body_rows = self.body_rows(width, height);
        if let Some(offset) = rail_offset(row, column, width, body_rows) {
            // The list keeps the selected row on its last line, so the row
            // under the pointer is that offset plus the window's first session.
            let index = self.session_top(body_rows) + offset;
            // A row past the end of the list names nothing, and clicking the
            // open session is not a change: it is the state the rail already
            // shows.
            if index >= self.sessions.len() || index == self.selected {
                return;
            }
            self.select(index);
            return;
        }
        let Some(offset) = transcript_offset(row, column, width, body_rows) else {
            return;
        };
        // The frame and the pointer read the same document, so the row under
        // the pointer names the call that row paints - or nothing, if the row
        // is prose, blank, or part of a pane.
        let (_, hits) = self.transcript(transcript_width(width), body_rows);
        let Some(Some(index)) = hits.get(offset).copied() else {
            return;
        };
        // Clicking the call the pane already shows is not a change.
        if self.detail == Some(index) {
            return;
        }
        self.open_detail(index);
    }

    /// Route a click to the open choice list, and report whether it was the
    /// list's to take.
    ///
    /// The list is drawn as the rows above the composer, so a pointer on one of
    /// them means the choice that row paints - the same choice Enter commits.
    /// A click outside the list's rows leaves it open and answers `false`, so
    /// the rail beside it still opens a session.
    fn click_picker(&mut self, row: usize) -> bool {
        if self.picker.is_none() {
            return false;
        }
        let Some(picker) = self.picker.as_mut() else {
            return false;
        };
        let (start, visible) = picker.window();
        let shown = picker.rows().len();
        let Some(top) = self.picker_at.get() else {
            return false;
        };
        if let Some(index) = list_offset(row, top, 1, start, visible, shown) {
            picker.select(index);
            // The commit reads the highlight, so the two gestures share one
            // path: a pick is the choice the row names, not a second code path
            // that could resolve it differently.
            self.commit_picker();
            return true;
        }
        // A click on the list's title or its key line names no choice, but it
        // is still the list's: the transcript under it has no row there to
        // open, and the list must not close under a reader who missed a row.
        row >= top
    }

    /// Route a click to the open completion popup, and report whether it was
    /// the popup's to take.
    ///
    /// The popup is drawn as the rows above the composer with no header of its
    /// own, so a pointer on one of them means the candidate that row paints -
    /// the same candidate Enter accepts. A click outside those rows leaves the
    /// popup open and answers `false`, so the rail and the transcript beside
    /// it still answer a pointer.
    fn click_menu(&mut self, row: usize) -> bool {
        if !self.menu.open() {
            return false;
        }
        let (start, visible) = self.menu.window();
        let shown = self.menu.rows().len();
        let Some(top) = self.menu_at.get() else {
            return false;
        };
        if let Some(index) = list_offset(row, top, 0, start, visible, shown) {
            self.menu.select(index);
            // The accept reads the highlight, so the two gestures share one
            // path: a pick is the row the pointer named, not a second code
            // path that could resolve it differently.
            self.accept();
            return true;
        }
        false
    }

    /// Ask for the reader's own editor on the prompt as it stands.
    ///
    /// The text is written to a file by the loop that owns the terminal, so
    /// this only records the request: a key handler that blocked on a program
    /// the reader is typing into would be the wrong side of the redraw.
    fn edit_composer(&mut self) {
        self.edit_request = Some(self.input.clone());
        self.set_status(Level::Info, "opening the editor the reader configured");
    }

    fn submit(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return;
        }
        // Every submitted line is recalled with Up/Down, commands included:
        // retyping a slash line is the same nuisance as retyping a turn.
        self.history.record(&text);
        // A slash line changes the session's configuration; it is never sent
        // to the model, the way the desktop's own commands are not.
        if let Some(command) = SlashCommand::parse(&text) {
            self.input.clear();
            self.cursor = 0;
            self.run_command(command);
            return;
        }
        if self.deliver(&text) {
            self.input.clear();
            self.cursor = 0;
        }
    }

    /// Send one line to the open session, or hold it in the queue while a turn
    /// runs. Returns whether the Host (or the queue) took it; a refusal leaves
    /// the caller's text in place so a retry is possible.
    ///
    /// The composer and the question card both end here: a choice the reader
    /// picked becomes the same text as one they typed, so the two gestures
    /// share one send path rather than two that could drift apart.
    fn deliver(&mut self, text: &str) -> bool {
        let Some(session_id) = self.current_session() else {
            self.set_status(Level::Error, "no session to send to; Ctrl-T creates one");
            return false;
        };
        // The desktop holds the lines typed during a turn in its queue dock
        // rather than starting a second execution; the terminal keeps the same
        // inbox and sends the head of it when the Host reports the session idle.
        if self.busy() {
            if !self.queue.push(session_id, text) {
                self.set_status(
                    Level::Error,
                    "the queue belongs to another session; /queue clear drops it",
                );
                return false;
            }
            self.set_status(
                Level::Info,
                format!(
                    "queued ({} waiting); it sends when the running turn ends",
                    self.queue.len()
                ),
            );
            return true;
        }
        match self.client.send_message(session_id, text.to_string()) {
            Ok(_) => {
                self.scroll = 0;
                self.set_status(Level::Info, "turn accepted by the Host");
                self.refresh_all();
                true
            }
            Err(error) => {
                self.client_error(error);
                false
            }
        }
    }

    /// Open the card the newest `ask_user` call left unanswered, or put it
    /// away once the reader's own turn answered it.
    ///
    /// The projection decides, not an event: the call's arguments are the
    /// questions, and a user message after the call is the answer - the same
    /// two facts the desktop settles its card on.
    fn sync_question(&mut self) {
        let Some(session) = self.session.as_ref() else {
            self.question = None;
            return;
        };
        let Some((call, questions)) = pending_question(session) else {
            self.question = None;
            return;
        };
        if self.question_dismissed == Some(call) {
            self.question = None;
            return;
        }
        if self.question.as_ref().is_some_and(|card| card.call == call) {
            // The card is already open on this call; re-opening it would throw
            // away the picks the reader has made so far.
            return;
        }
        self.question = Some(QuestionCard::new(call, questions));
    }

    /// The card's rows for this frame, empty when it is closed.
    fn card_rows(&self, width: usize) -> Vec<String> {
        self.question
            .as_ref()
            .map(|card| question_lines(card, width))
            .unwrap_or_default()
    }

    /// The card's keys, while it is open and the prompt is empty.
    ///
    /// Returns whether the card consumed the key. An empty prompt is the
    /// reader's focus on the card; the moment anything is typed the composer
    /// takes every key back, so a reader answering in their own words is never
    /// competing with a highlight.
    fn question_key(&mut self, key: &Key) -> bool {
        if self.question.is_none() || !self.input.is_empty() {
            return false;
        }
        match key {
            Key::Up | Key::ScrollUp => {
                if let Some(card) = self.question.as_mut() {
                    card.move_by(-1);
                }
                true
            }
            Key::Down | Key::ScrollDown => {
                if let Some(card) = self.question.as_mut() {
                    card.move_by(1);
                }
                true
            }
            // A digit moves the highlight instead of picking: a reader typing
            // the first character of an answer must never send one it did not
            // choose, so the pick always takes one more key press.
            Key::Char(digit) if digit.is_ascii_digit() => {
                if let Some(card) = self.question.as_mut() {
                    card.jump(*digit);
                }
                true
            }
            Key::Enter => {
                self.pick_question();
                true
            }
            Key::Newline => {
                self.advance_question();
                true
            }
            Key::Escape => {
                self.dismiss_question();
                true
            }
            _ => false,
        }
    }

    /// Enter on the highlighted option: a single-select step takes the pick and
    /// moves on, a multi-select one toggles it and waits for Ctrl-J.
    fn pick_question(&mut self) {
        let Some(multi) = self.question.as_mut().and_then(QuestionCard::pick) else {
            return;
        };
        if multi {
            let picked = self
                .question
                .as_ref()
                .and_then(|card| card.picks.get(card.step).map(Vec::len))
                .unwrap_or(0);
            self.set_status(
                Level::Info,
                format!("{picked} option(s) picked · Ctrl-J moves on"),
            );
            return;
        }
        self.advance_question();
    }

    /// Move to the next step, or send the answers when this was the last one.
    ///
    /// A multi-select step with no pick is refused, the way the desktop's own
    /// Next button stays disabled until the reader picks something: an empty
    /// step would tell the model nothing about the question it asked.
    fn advance_question(&mut self) {
        let blocked = match self.question.as_ref() {
            Some(card) => {
                card.questions
                    .get(card.step)
                    .is_some_and(|question| question.multi_select)
                    && card.picks.get(card.step).is_none_or(Vec::is_empty)
            }
            None => return,
        };
        if blocked {
            self.set_status(Level::Error, "pick at least one option before moving on");
            return;
        }
        if self.question.as_mut().is_some_and(QuestionCard::advance) {
            self.settle_question();
        }
    }

    /// Send every step's picks as one message and put the card away.
    ///
    /// The answers are the desktop's own shape, so the model receives the same
    /// text whichever interface asked. A line the Host refuses brings the card
    /// back with its picks: retyping an answer is worse than it waiting.
    fn settle_question(&mut self) {
        let Some(card) = self.question.take() else {
            return;
        };
        let answer = card.answer();
        self.question_dismissed = Some(card.call);
        self.history.record(&answer);
        if self.deliver(&answer) {
            return;
        }
        self.question = Some(card);
        self.question_dismissed = None;
    }

    /// Put the card away without answering it.
    ///
    /// The transcript keeps the tool call, so the questions stay readable, and
    /// the reader can answer in their own words; `/questions` brings the card
    /// back while the call is still unanswered.
    fn dismiss_question(&mut self) {
        let Some(card) = self.question.take() else {
            return;
        };
        self.question_dismissed = Some(card.call);
        self.set_status(
            Level::Info,
            "question set aside; type your own answer, or /questions to come back",
        );
    }

    /// Bring a set-aside card back, or say the session is not waiting on one.
    fn questions_command(&mut self) {
        self.question_dismissed = None;
        self.sync_question();
        let summary = self.question.as_ref().map(|card| {
            format!(
                "question {} of {} step(s) · {} option(s) offered",
                card.call + 1,
                card.questions.len(),
                card.current().map_or(0, |question| question.options.len())
            )
        });
        match summary {
            Some(summary) => self.set_status(Level::Info, summary),
            None => self.set_status(Level::Info, "no unanswered question in this session"),
        }
    }

    /// Run one configuration command against the Host.
    fn run_command(&mut self, command: SlashCommand) {
        // A command that opens a document replaces whatever document is open.
        // The body is one column, so a pane left behind a newer one would come
        // back - stale - when the newer one closed. The invariant lives here so
        // every opening command inherits it, rather than each one remembering
        // the full list of panes.
        if command.opens_pane() {
            self.close_panes();
        }
        match command {
            SlashCommand::Help => self.set_status(Level::Info, COMMANDS),
            SlashCommand::New(title) => self.create_session(title),
            // The prompt goes to the reader's own editor, which is a change to
            // client state only: nothing is sent until Enter sends it.
            SlashCommand::Editor => self.edit_composer(),
            SlashCommand::Sessions(argument) => self.sessions_command(argument.as_deref()),
            SlashCommand::Skills(filter) => self.skills_command(filter.as_deref()),
            SlashCommand::Unknown(head) => {
                self.set_status(
                    Level::Error,
                    format!("unknown command '{head}' — {COMMANDS}"),
                );
            }
            SlashCommand::Usage(usage) => self.set_status(Level::Error, usage),
            // The three commands whose argument is one of a fixed set answer a
            // reader who typed only the command with a list. The desktop opens
            // a picker for each of them; a terminal's names-in-a-row asked the
            // reader to retype one, which is the work a list should do.
            SlashCommand::Model(None, _) => self.open_picker(picker::Kind::Model),
            SlashCommand::Model(Some(provider), Some(model)) => self.configure(
                SessionConfigChange {
                    provider: Some(provider),
                    model: Some(model),
                    ..SessionConfigChange::default()
                },
                |config| config.model_ref().is_some(),
            ),
            SlashCommand::Model(Some(_), None) => {
                self.set_status(
                    Level::Error,
                    "usage: /model <provider> <model>; /model alone opens the list",
                );
            }
            SlashCommand::Preset(None) => self.open_picker(picker::Kind::Preset),
            SlashCommand::Preset(Some(preset)) => self.configure(
                SessionConfigChange {
                    preset: Some(preset),
                    ..SessionConfigChange::default()
                },
                |config| config.preset.is_some(),
            ),
            SlashCommand::Permission(None) => self.open_picker(picker::Kind::Permission),
            SlashCommand::Permission(Some(permission)) => {
                // The menu offers only the modes the contract names; a typo is
                // answered here instead of being sent as a permission the
                // Host would have to refuse.
                if !PERMISSION_MODES.contains(&permission.as_str()) {
                    self.set_status(
                        Level::Error,
                        format!(
                            "unknown permission '{permission}'; one of {}",
                            PERMISSION_MODES.join(", ")
                        ),
                    );
                    return;
                }
                self.configure(
                    SessionConfigChange {
                        permission: Some(permission),
                        ..SessionConfigChange::default()
                    },
                    |config| !config.permission.trim().is_empty(),
                );
            }
            SlashCommand::Plan(None) => self.toggle_plan(),
            SlashCommand::Plan(Some(plan)) => self.configure(
                SessionConfigChange {
                    plan_mode: Some(plan),
                    ..SessionConfigChange::default()
                },
                |config| config.plan_mode == plan,
            ),
            SlashCommand::Compact => self.compact(),
            SlashCommand::Fork => self.fork_session(),
            SlashCommand::Copy(count) => self.copy_message(count.unwrap_or(1)),
            SlashCommand::CopyId => self.copy_session_id(),
            SlashCommand::Approve => self.decide_approval(true, None),
            SlashCommand::Deny(note) => self.decide_approval(false, note),
            SlashCommand::GoalShow => self.show_goal(),
            SlashCommand::GoalSet(objective) => self.set_goal(objective),
            SlashCommand::GoalEdit(objective) => self.edit_goal(objective),
            SlashCommand::GoalPause => self.pause_goal(),
            SlashCommand::GoalResume => self.resume_goal(),
            SlashCommand::GoalClear => self.clear_goal(),
            SlashCommand::Rename(None) => {
                let current = self
                    .session
                    .as_ref()
                    .map(|session| session.title.clone())
                    .unwrap_or_else(|| "no session".to_string());
                self.set_status(
                    Level::Info,
                    format!("title: {current} · usage: /rename <title>"),
                );
            }
            SlashCommand::Rename(Some(title)) => self.rename(title),
            SlashCommand::Export(path) => self.export(path),
            SlashCommand::Queue(argument) => self.queue_command(argument.as_deref()),
            SlashCommand::Feedback(argument) => self.feedback_command(argument.as_deref()),
            SlashCommand::Detail(argument) => self.detail_command(argument.as_deref()),
            SlashCommand::UsageShow => self.usage_command(),
            SlashCommand::Mcp(argument) => self.mcp_command(argument.as_deref()),
            SlashCommand::Memory(argument) => self.memory_command(argument.as_deref()),
            SlashCommand::Files(argument) => self.files_command(argument.as_deref()),
            SlashCommand::Diff(argument) => self.diff_command(argument.as_deref()),
            SlashCommand::Tasks(id) => self.tasks_command(id.as_deref()),
            SlashCommand::Extensions(argument) => self.extensions_command(argument.as_deref()),
            SlashCommand::Federation => self.federation_command(),
            SlashCommand::Browser => self.browser_command(),
            SlashCommand::Subagents(argument) => self.subagents_command(argument.as_deref()),
            SlashCommand::Trajectory => self.trajectory_command(),
            SlashCommand::Review(argument) => self.review_command(argument.as_deref()),
            SlashCommand::Trash(argument) => self.trash_command(argument.as_deref()),
            SlashCommand::Restore(argument) => self.restore_command(argument.as_deref()),
            SlashCommand::Purge(argument) => self.purge_command(argument.as_deref()),
            SlashCommand::Terminal => self.terminal_command(),
            SlashCommand::Questions => self.questions_command(),
            SlashCommand::Settings(argument) => self.settings_command(argument.as_deref()),
            SlashCommand::Set { key, value } => self.set_command(key.as_deref(), value.as_deref()),
            SlashCommand::Audit(argument) => self.audit_command(argument.as_deref()),
            SlashCommand::Find(text) => self.find_command(&text),
        }
    }

    /// The MCP servers this Host's build connected, and what each offers.
    ///
    /// The roster is the Host's, not this client's guess: the query is asked
    /// again here so a server an operator fixed in the settings a moment ago is
    /// reported from the Host's pool rather than from the copy this client
    /// fetched when it attached. Every state the desktop pane distinguishes is
    /// named: disabled, connected with its tools, or not connected and why.
    fn mcp_command(&mut self, argument: Option<&str>) {
        match self.client.catalogue() {
            Ok(catalogue) => self.catalogue = catalogue,
            Err(error) => {
                self.client_error(error);
                return;
            }
        }
        let servers = &self.catalogue.mcp;
        if servers.is_empty() {
            self.set_status(
                Level::Info,
                "no mcp servers are configured in bsh settings or installed extensions",
            );
            return;
        }
        let Some(argument) = argument
            .map(str::trim)
            .filter(|argument| !argument.is_empty())
        else {
            let rows: Vec<String> = servers.iter().map(mcp_row).collect();
            self.set_status(Level::Info, format!("mcp: {}", rows.join(" · ")));
            return;
        };
        let Some(server) = servers
            .iter()
            .find(|server| server.name.eq_ignore_ascii_case(argument))
        else {
            let names: Vec<&str> = servers.iter().map(|server| server.name.as_str()).collect();
            self.set_status(
                Level::Error,
                format!(
                    "no mcp server named '{argument}'; known: {}",
                    names.join(", ")
                ),
            );
            return;
        };
        let level = if server.error.is_some() {
            Level::Error
        } else {
            Level::Info
        };
        self.set_status(level, mcp_detail(server));
    }

    /// The skills this build registered, and what each one does.
    ///
    /// The catalogue is asked again, like the MCP roster beside it: the rows the
    /// `/` list ranks and the gesture a turn resolves are one resolution, so a
    /// skill installed while the terminal was open is listed from the Host
    /// rather than from the copy this client attached with. A workspace that
    /// installed none says so; the desktop palette draws no section for it.
    fn skills_command(&mut self, argument: Option<&str>) {
        match self.client.catalogue() {
            Ok(catalogue) => self.catalogue = catalogue,
            Err(error) => {
                self.client_error(error);
                return;
            }
        }
        let skills = &self.catalogue.skills;
        if skills.is_empty() {
            self.set_status(Level::Info, "no skills are registered in this workspace");
            return;
        }
        let Some(filter) = argument.map(str::trim).filter(|filter| !filter.is_empty()) else {
            let rows: Vec<String> = skills.iter().map(skill_row).collect();
            self.set_status(
                Level::Info,
                format!("{} skills: {}", skills.len(), rows.join(" · ")),
            );
            return;
        };
        // One exact name opens the skill; anything else is a search of the
        // palette's own kind, because a reader remembers a name but may only
        // half-remember what it does.
        if let Some(skill) = skills
            .iter()
            .find(|skill| skill.name.eq_ignore_ascii_case(filter))
        {
            self.set_status(Level::Info, skill_detail(skill));
            return;
        }
        let hits: Vec<&SkillView> = skills
            .iter()
            .filter(|skill| skill_matches(skill, filter))
            .collect();
        match hits.as_slice() {
            [] => {
                let names: Vec<&str> = skills.iter().map(|skill| skill.name.as_str()).collect();
                self.set_status(
                    Level::Error,
                    format!("no skill matches '{filter}'; known: {}", names.join(", ")),
                );
            }
            [skill] => self.set_status(Level::Info, skill_detail(skill)),
            many => {
                let rows: Vec<String> = many.iter().map(|skill| skill_row(skill)).collect();
                self.set_status(
                    Level::Info,
                    format!("skills matching '{filter}': {}", rows.join(" · ")),
                );
            }
        }
    }

    /// The session's token buckets, with the exact figures the desktop's token
    /// dialog prints under its usage pill.
    ///
    /// The figures are summed from the projection, not counted here: every
    /// assistant row the Host committed carries the usage its turn reported, so
    /// a client that attached after the turn reads the same total a client that
    /// watched it does.
    fn usage_command(&mut self) {
        let Some(session) = self.session.as_ref() else {
            self.set_status(Level::Error, "no session to total");
            return;
        };
        let Some(usage) = session_usage(session) else {
            self.set_status(
                Level::Info,
                "no turn in this session reported usage; the provider sent no figures",
            );
            return;
        };
        let total = crate::state::exact_tokens(usage.total());
        let input = crate::state::exact_tokens(usage.uncached_input());
        let cached = crate::state::exact_tokens(usage.cached_tokens);
        let output = crate::state::exact_tokens(usage.completion_tokens);
        let percent = crate::state::cache_hit_percent(usage.cached_tokens, usage.prompt_tokens)
            .map(|percent| format!(" · cache hit {percent}%"))
            .unwrap_or_default();
        self.set_status(
            Level::Info,
            format!(
                "usage {total} tokens{percent} · input {input} · cache read {cached} · output {output}"
            ),
        );
    }

    /// Open or descend the Files pane: the terminal's form of the desktop's
    /// Workspace Files browser.
    ///
    /// One query answers both a directory and a file, so the Host's root stays
    /// the only thing a path is resolved against: this client never joins a path
    /// itself, and a file the desktop editor refuses for being binary or
    /// oversized is refused here by the same rule. The report is asked for on
    /// every call rather than cached, because a turn writes files while the
    /// reader is browsing.
    fn files_command(&mut self, argument: Option<&str>) {
        let path = argument.map(str::trim).unwrap_or_default();
        match self.client.files(path) {
            Ok(report) => {
                let summary = files_summary(&report);
                self.settings = None;
                self.files = Some(FilesPane {
                    report,
                    selection: 0,
                    scroll: 0,
                });
                // One pane at a time, like `/memory`, `/audit` and `/detail`: the
                // reader asked to browse the workspace, so the transcript yields.
                self.memory = None;
                self.audit = None;
                self.detail = None;
                self.tasks = None;
                self.extensions = None;
                self.review = None;
                self.federation = None;
                self.trajectory = None;
                self.set_status(Level::Info, summary);
            }
            Err(error) => self.client_error(error),
        }
    }

    /// Leave the Files pane and keep whatever the composer holds.
    fn close_files(&mut self) {
        self.files = None;
        self.set_status(Level::Info, "files closed");
    }

    /// Walk the highlighted row, or scroll a document.
    ///
    /// The pane clamps both: a selection never points past the last row of a
    /// directory that shrank under it, and a short file never scrolls away from
    /// its first line.
    fn move_files(&mut self, delta: isize) {
        let Some(pane) = self.files.as_mut() else {
            return;
        };
        if files_is_listing(&pane.report) {
            let len = pane.report.entries.len();
            if len > 0 {
                pane.selection = pane.selection.saturating_add_signed(delta).min(len - 1);
            }
        } else {
            pane.scroll = pane.scroll.saturating_add_signed(delta);
        }
    }

    /// Open the highlighted row: a directory descends and a file is read, both by
    /// the one query. A document has no rows to walk, so Enter climbs back to the
    /// directory it came from - the gesture the desktop gets from its breadcrumb.
    fn open_files_selection(&mut self) {
        let Some(pane) = self.files.as_ref() else {
            return;
        };
        if !files_is_listing(&pane.report) {
            let parent = parent_path(&pane.report.path);
            self.files_command(Some(&parent));
            return;
        }
        let len = pane.report.entries.len();
        let Some(entry) = pane
            .report
            .entries
            .get(pane.selection.min(len.saturating_sub(1)))
        else {
            let place = files_place(&pane.report).to_string();
            self.set_status(Level::Info, format!("{place} holds nothing to open"));
            return;
        };
        let target = join_path(&pane.report.path, &entry.name);
        self.files_command(Some(&target));
    }

    /// Open or walk the Diff pane: the terminal's form of the desktop Diff
    /// tab.
    ///
    /// No argument asks the Host what the working copy changed; a path asks for
    /// that file's comparison. The query is sent on every call rather than
    /// cached, because a turn writes files while the reader is looking at them,
    /// so the list a reader opens is the list the Host holds now. One pane at a
    /// time, like `/files`: the reader asked what changed, so whatever they had
    /// open yields.
    fn diff_command(&mut self, argument: Option<&str>) {
        let path = argument.map(str::trim).unwrap_or_default();
        match self.client.diff(path) {
            Ok(view) => {
                let summary = diff_summary(&view);
                let refused = diff_refusal(&view);
                self.settings = None;
                self.diff = Some(DiffPane {
                    view,
                    selection: 0,
                    scroll: 0,
                });
                // One pane at a time, like `/files` and `/review`: the reader
                // asked to compare the working copy, so the transcript yields.
                self.memory = None;
                self.audit = None;
                self.detail = None;
                self.tasks = None;
                self.extensions = None;
                self.review = None;
                self.federation = None;
                self.browser = None;
                self.trajectory = None;
                self.files = None;
                let level = if refused { Level::Error } else { Level::Info };
                self.set_status(level, summary);
            }
            Err(error) => self.client_error(error),
        }
    }

    /// Leave the Diff pane and keep whatever the composer holds.
    fn close_diff(&mut self) {
        self.diff = None;
        self.set_status(Level::Info, "diff closed");
    }

    /// Walk the highlighted change, or scroll a patch.
    ///
    /// A listing has rows, so Up and Down walk them and the selection is
    /// clamped against a list that shrank under it. A patch has no rows, so the
    /// same keys scroll it.
    fn move_diff(&mut self, delta: isize) {
        let Some(pane) = self.diff.as_mut() else {
            return;
        };
        if diff_is_patch(&pane.view) {
            pane.scroll = pane.scroll.saturating_add_signed(delta);
            return;
        }
        let len = pane.view.changes.len();
        if len > 0 {
            pane.selection = pane.selection.saturating_add_signed(delta).min(len - 1);
        }
    }

    /// Open the highlighted change's comparison: one Enter asks the Host for
    /// that path, which is the gesture the desktop makes when it selects a
    /// changed file in the Files tree. A pane already showing a patch has no
    /// rows, so Enter goes back to the list - the gesture the files pane uses
    /// to climb out of a document.
    fn open_diff_selection(&mut self) {
        let Some(pane) = self.diff.as_ref() else {
            return;
        };
        if diff_is_patch(&pane.view) {
            self.diff_command(None);
            return;
        }
        let len = pane.view.changes.len();
        let Some(change) = pane
            .view
            .changes
            .get(pane.selection.min(len.saturating_sub(1)))
        else {
            self.set_status(Level::Info, diff_summary(&pane.view));
            return;
        };
        let path = change.path.clone();
        self.diff_command(Some(&path));
    }

    /// Leave the Trajectory pane and keep whatever the composer holds.
    fn close_trajectory(&mut self) {
        self.trajectory = None;
        self.set_status(Level::Info, "trajectory closed");
    }

    /// Walk the log: `delta` is measured in rows back from the newest one, so a
    /// positive value moves up into older events and a negative one returns
    /// toward the live tail. Zero follows the tail again, which is where a
    /// reader who scrolled all the way down expects to land.
    fn scroll_trajectory(&mut self, delta: isize) {
        let Some(pane) = self.trajectory.as_mut() else {
            return;
        };
        pane.back = pane.back.saturating_add_signed(delta);
    }

    /// Open the session's trajectory.
    ///
    /// The rows are derived from the projection this client already holds, the
    /// way the desktop's Trajectory tab is built from the same transcript and
    /// call records: no query is sent, and a turn that is still running keeps
    /// appending to the log as its events are committed. One pane at a time,
    /// like `/audit` and `/detail`.
    fn trajectory_command(&mut self) {
        if self.session.is_none() {
            self.set_status(Level::Info, "no session open — Ctrl-T creates one");
            return;
        }
        let summary = trajectory_summary(self.session.as_ref());
        self.settings = None;
        self.trajectory = Some(TrajectoryPane { back: 0 });
        self.memory = None;
        self.files = None;
        self.diff = None;
        self.tasks = None;
        self.audit = None;
        self.detail = None;
        self.extensions = None;
        self.review = None;
        self.federation = None;
        self.set_status(Level::Info, summary);
    }

    /// Read the workspace's durable task board and open the Tasks pane.
    ///
    /// The query is asked on every open rather than cached from attach: the
    /// board is the platform's live record set (a turn's task tools write it
    /// while this client is attached), and a reader who opened the pane asked
    /// what is running now. One pane at a time, like `/memory` and `/files`.
    ///
    /// The argument names a task, or the word `graph`: the desktop's Tasks pane
    /// has two views - the board and the graph over it - and a terminal reaches
    /// the second by naming it, because a terminal has no tab strip to click.
    fn tasks_command(&mut self, argument: Option<&str>) {
        let argument = argument
            .map(str::trim)
            .filter(|argument| !argument.is_empty());
        // A task id is never the word, so the keyword wins over a lookup.
        let graph = argument.is_some_and(|argument| argument.eq_ignore_ascii_case("graph"));
        let id = argument.filter(|_| !graph);
        let detail = match id {
            Some(id) => match self.client.task_detail(id) {
                Ok(detail) => Some(detail),
                Err(error) => {
                    self.client_error(error);
                    return;
                }
            },
            None => None,
        };
        match self.client.tasks() {
            Ok(report) => {
                let summary = match (detail.as_ref(), graph) {
                    (Some(detail), _) => task_detail_summary(detail),
                    (None, true) => tasks_graph_summary(&report),
                    (None, false) => tasks_summary(&report),
                };
                let level = match detail.as_ref() {
                    Some(detail) if detail.state != "ready" => Level::Error,
                    _ => Level::Info,
                };
                self.settings = None;
                self.tasks = Some(TasksPane {
                    report,
                    detail,
                    graph,
                    scroll: 0,
                });
                self.memory = None;
                self.files = None;
                self.diff = None;
                self.audit = None;
                self.detail = None;
                self.extensions = None;
                self.review = None;
                self.federation = None;
                self.trajectory = None;
                self.set_status(level, summary);
            }
            Err(error) => self.client_error(error),
        }
    }

    /// Leave the Tasks pane and keep whatever the composer holds.
    fn close_tasks(&mut self) {
        self.tasks = None;
        self.set_status(Level::Info, "tasks closed");
    }

    /// Read the node's identity and peer roster and open the Federation pane.
    ///
    /// The query is asked on every open rather than cached from attach: a peer
    /// can be enrolled, revoked or expire while this client is attached, and a
    /// reader who opened the pane asked who is trusted now. The rows are the
    /// executor's own view of the node and the peer store, so the peer a
    /// terminal names and the grants it lists are the ones the desktop's
    /// Federation tab draws. One pane at a time, like `/tasks` and
    /// `/extensions`.
    fn federation_command(&mut self) {
        match self.client.federation() {
            Ok(report) => {
                let summary = federation_summary(&report);
                self.settings = None;
                self.federation = Some(FederationPane { report, scroll: 0 });
                self.memory = None;
                self.files = None;
                self.diff = None;
                self.tasks = None;
                self.audit = None;
                self.detail = None;
                self.extensions = None;
                self.review = None;
                self.set_status(Level::Info, summary);
            }
            Err(error) => self.client_error(error),
        }
    }

    /// Leave the Federation pane and keep whatever the composer holds.
    fn close_federation(&mut self) {
        self.federation = None;
        self.set_status(Level::Info, "federation closed");
    }

    /// Walk the roster: the body is a document, so the offset is clamped where
    /// it is read and a short roster never scrolls away from its top.
    fn scroll_federation(&mut self, delta: isize) {
        let Some(pane) = self.federation.as_mut() else {
            return;
        };
        pane.scroll = pane.scroll.saturating_add_signed(delta);
    }

    /// Read the Host browser's page and open the Browser pane.
    ///
    /// The page is the one the agent's browser tools left behind: a turn that
    /// visited a page and read it is what this shows, the same document the
    /// desktop's Browser tab draws as text. Asking on every open rather than
    /// caching is what makes a turn's page appear under a client that was
    /// attached while it ran. One pane at a time, like `/federation`.
    fn browser_command(&mut self) {
        match self.client.browser() {
            Ok(view) => {
                let summary = browser_summary(&view);
                self.settings = None;
                self.browser = Some(BrowserPane { view, scroll: 0 });
                self.memory = None;
                self.files = None;
                self.diff = None;
                self.tasks = None;
                self.audit = None;
                self.detail = None;
                self.extensions = None;
                self.review = None;
                self.federation = None;
                self.trajectory = None;
                self.set_status(Level::Info, summary);
            }
            Err(error) => self.client_error(error),
        }
    }

    /// Close every pane the body can show.
    ///
    /// The body is one column, so opening a document replaces whatever was
    /// there. Keeping that invariant in one place is what stops a pane from
    /// surviving behind a newer one and reappearing - stale - when the newer
    /// one closes.
    fn close_panes(&mut self) {
        self.settings = None;
        self.trajectory = None;
        self.tasks = None;
        self.federation = None;
        self.browser = None;
        self.subagents = None;
        self.files = None;
        self.diff = None;
        self.audit = None;
        self.extensions = None;
        self.review = None;
        self.memory = None;
        self.detail = None;
        self.detail_scroll = 0;
    }

    /// Leave the Browser pane and keep whatever the composer holds.
    fn close_browser(&mut self) {
        self.browser = None;
        self.set_status(Level::Info, "browser closed");
    }

    /// Walk the page: its text is a document, so the offset is clamped where
    /// it is read and a short page never scrolls away from its top.
    fn scroll_browser(&mut self, delta: isize) {
        let Some(pane) = self.browser.as_mut() else {
            return;
        };
        pane.scroll = pane.scroll.saturating_add_signed(delta);
    }

    /// The delegated child runs this view answers for.
    ///
    /// The desktop draws these as nodes under the conversation in its Tasks
    /// graph and counts them in the header, from the registry the `subagent`
    /// tool writes inside the turn's own process. A client that attached to the
    /// Host has no registry to read, so this pane is a query rather than a
    /// count of the transcript the way `/audit` is.
    ///
    /// No argument reads the open session's lineage - the children this
    /// conversation delegated - `all` reads every run the Host holds, and a
    /// number opens one row's prompt and answer. The roster is re-read while a
    /// child still runs ([`Self::poll_subagents`]), because a run finishes
    /// without any event this client subscribed to.
    fn subagents_command(&mut self, argument: Option<&str>) {
        let argument = argument.map(str::trim).filter(|text| !text.is_empty());
        let all = argument.is_some_and(|text| text.eq_ignore_ascii_case("all"));
        let number = match argument {
            None => None,
            Some(_) if all => None,
            Some(text) => match text.parse::<usize>() {
                Ok(number) => Some(number),
                Err(_) => {
                    self.set_status(
                        Level::Error,
                        format!("'{text}' is not a run number · usage: /subagents [number|all]"),
                    );
                    return;
                }
            },
        };
        // The conversation is the filter the desktop's graph uses: a reader who
        // opened a chat asked about that chat's children, and only `all` asks
        // the Host for every run it holds.
        let scope = if all {
            None
        } else {
            self.session.as_ref().map(|session| session.session_id)
        };
        let report = match self.client.subagents(scope) {
            Ok(report) => report,
            Err(error) => {
                self.client_error(error);
                return;
            }
        };
        let summary = subagents_summary(&report, all);
        let mut pane = SubagentsPane {
            report,
            selection: 0,
            open: None,
            scroll: 0,
            ticks: 0,
            scope,
        };
        if let Some(number) = number {
            let total = pane.report.runs.len();
            if number == 0 || number > total {
                self.set_status(
                    Level::Error,
                    format!("no run {number}; /subagents lists {total} in this view"),
                );
                return;
            }
            pane.selection = number - 1;
            pane.open = Some(number - 1);
        }
        self.subagents = Some(pane);
        self.set_status(Level::Info, summary);
    }

    /// Leave the child-run pane, closing the run opened inside it first.
    ///
    /// One Escape unwinds one document, the way the files pane climbs out of a
    /// file into its directory: a reader who opened a run to read its answer is
    /// not also asking for the roster to go away.
    fn close_subagents(&mut self) {
        let Some(pane) = self.subagents.as_mut() else {
            return;
        };
        if pane.open.take().is_some() {
            pane.scroll = 0;
            self.set_status(Level::Info, "closed the run · /subagents lists the roster");
            return;
        }
        self.subagents = None;
        self.set_status(Level::Info, "subagents closed");
    }

    /// Walk the roster, or scroll the run that is open: the pane decides which,
    /// the way the files pane decides between a listing and a document.
    fn move_subagents(&mut self, delta: isize) {
        let Some(pane) = self.subagents.as_mut() else {
            return;
        };
        if pane.open.is_some() {
            pane.scroll = pane.scroll.saturating_add_signed(delta);
            return;
        }
        let len = pane.report.runs.len();
        if len > 0 {
            pane.selection = pane.selection.saturating_add_signed(delta).min(len - 1);
        }
    }

    /// Open the highlighted run: the task the child was given and the answer it
    /// produced, which a terminal has no hover card to show.
    fn open_subagents_selection(&mut self) {
        let Some(pane) = self.subagents.as_mut() else {
            return;
        };
        if pane.report.runs.is_empty() {
            self.set_status(Level::Info, "no child run to open in this view");
            return;
        }
        pane.selection = pane.selection.min(pane.report.runs.len() - 1);
        pane.open = Some(pane.selection);
        pane.scroll = 0;
        let number = pane.selection + 1;
        let title = pane.report.runs[pane.selection].title.trim().to_string();
        self.set_status(Level::Info, format!("run {number} · {title} · Esc closes"));
    }

    /// Re-read an open roster while a child still runs.
    ///
    /// A child's state moves inside the Host's process and no event carries it
    /// to this client, so the pane asks again on a slow tick: a run that
    /// finished shows the answer it produced without the reader re-opening the
    /// pane. A dropped re-read keeps the rows that are on screen - the next
    /// tick asks again, and a refused query is not a reason to blank a document
    /// the reader is reading.
    fn poll_subagents(&mut self) {
        let Some(scope) = self.subagents.as_ref().map(|pane| pane.scope) else {
            return;
        };
        let Ok(report) = self.client.subagents(scope) else {
            return;
        };
        if let Some(pane) = self.subagents.as_mut() {
            pane.report = report;
            let last = pane.report.runs.len().saturating_sub(1);
            pane.selection = pane.selection.min(last);
            pane.open = pane.open.filter(|open| *open < pane.report.runs.len());
        }
    }

    /// List the workspace's bin, or park one of the sessions `/sessions`
    /// numbers.
    ///
    /// The desktop parks a chat from its session menu; a terminal has no menu,
    /// so the session list's own row number is the handle: `a bare `/trash`
    /// lists what is parked, and `/trash 3` parks the third row `/sessions`
    /// shows. The number is read against a freshly queried list, so a session a
    /// turn created or renamed since the last `/sessions` cannot be missed.
    /// Parking is durable Host state, so the client only asks for it: the chat
    /// leaves every list and the snapshot, and a second client sees it go.
    fn trash_command(&mut self, argument: Option<&str>) {
        if let Err(error) = self.refresh() {
            self.client_error(error);
            return;
        }
        let Some(argument) = argument.map(str::trim).filter(|text| !text.is_empty()) else {
            if let Err(error) = self.refresh_trash() {
                self.client_error(error);
                return;
            }
            // The desktop's bin is a list whose rows carry a Restore action; a
            // terminal opens the same list, and Enter is that action, so a pick
            // and `/restore <n>` reach the Host by one path. A bin with nothing
            // parked keeps the sentence instead of an empty box.
            self.open_picker(picker::Kind::Trash);
            if self.picker.is_none() {
                let rows = self.trash_rows(Utc::now());
                let status = trash_summary(&rows, self.trash.len());
                self.set_status(Level::Error, status);
            }
            return;
        };
        let Ok(number) = argument.parse::<usize>() else {
            self.set_status(
                Level::Error,
                "usage: /trash [number]; /sessions lists the rows to park",
            );
            return;
        };
        if number == 0 || number > self.sessions.len() {
            self.set_status(
                Level::Error,
                format!(
                    "no session {number}; /sessions lists {} in this workspace",
                    self.sessions.len()
                ),
            );
            return;
        }
        let session = self.sessions[number - 1].clone();
        match self.client.trash_session(session.session_id) {
            Ok(_) => {
                self.set_status(
                    Level::Info,
                    format!(
                        "parked session {number}: {} · /trash lists the bin · /restore <n> takes it back",
                        session.title
                    ),
                );
                self.refresh_all();
                if let Err(error) = self.refresh_trash() {
                    self.client_error(error);
                }
            }
            // A refusal is the Host's, not this client's: a turn in flight is
            // the one reason parking is unsafe, and its words are the answer.
            Err(error) => self.client_error(error),
        }
    }

    /// Re-read the bin from the Host.
    ///
    /// The list is asked for on every use rather than kept from the last
    /// listing: another client, or this client's own turn, can park or restore
    /// a chat between one command and the next.
    fn refresh_trash(&mut self) -> Result<(), ClientError> {
        self.trash = self.client.trash()?;
        Ok(())
    }

    /// The numbered rows `/trash` prints, in the order `/restore` and `/purge`
    /// address them.
    fn trash_rows(&self, now: DateTime<Utc>) -> Vec<String> {
        self.trash
            .iter()
            .enumerate()
            .map(|(index, session)| session_row(index, session, false, now))
            .collect()
    }

    /// Resolve one bin row by the number `/trash` printed.
    ///
    /// The bin is re-read first, so the number is checked against the rows the
    /// Host holds now; `command` names the gesture to repeat in the usage line.
    fn trash_target(&mut self, argument: Option<&str>, command: &str) -> Option<SessionSummary> {
        if let Err(error) = self.refresh_trash() {
            self.client_error(error);
            return None;
        }
        let Some(argument) = argument.map(str::trim).filter(|text| !text.is_empty()) else {
            self.set_status(
                Level::Error,
                format!("usage: {command} <number>; /trash lists the bin"),
            );
            return None;
        };
        let Ok(number) = argument.parse::<usize>() else {
            self.set_status(
                Level::Error,
                format!("usage: {command} <number>; /trash lists the bin"),
            );
            return None;
        };
        if number == 0 || number > self.trash.len() {
            self.set_status(
                Level::Error,
                format!(
                    "no bin row {number}; /trash lists {} parked session(s)",
                    self.trash.len()
                ),
            );
            return None;
        };
        Some(self.trash[number - 1].clone())
    }

    /// Take one parked chat back into the lists.
    fn restore_command(&mut self, argument: Option<&str>) {
        let Some(session) = self.trash_target(argument, "/restore") else {
            return;
        };
        match self.client.restore_session(session.session_id) {
            Ok(_) => {
                self.set_status(
                    Level::Info,
                    format!(
                        "restored '{}' · it is back in /sessions with its transcript",
                        session.title
                    ),
                );
                self.refresh_all();
                if let Err(error) = self.refresh_trash() {
                    self.client_error(error);
                }
            }
            Err(error) => self.client_error(error),
        }
    }

    /// Drop one parked chat for good.
    ///
    /// There is no undo after this fact, which is why it takes the explicit
    /// number `/trash` printed rather than a filter or a name: the reader has
    /// to read the row it names.
    fn purge_command(&mut self, argument: Option<&str>) {
        let Some(session) = self.trash_target(argument, "/purge") else {
            return;
        };
        match self.client.purge_session(session.session_id) {
            Ok(_) => {
                self.set_status(
                    Level::Info,
                    format!("purged '{}' · the transcript is gone", session.title),
                );
                self.refresh_all();
                if let Err(error) = self.refresh_trash() {
                    self.client_error(error);
                }
            }
            Err(error) => self.client_error(error),
        }
    }

    /// Walk the board: the body is a document, so both offsets are clamped
    /// where they are read and a short board never scrolls away from its top.
    fn scroll_tasks(&mut self, delta: isize) {
        let Some(pane) = self.tasks.as_mut() else {
            return;
        };
        pane.scroll = pane.scroll.saturating_add_signed(delta);
    }

    /// Read the extension host's roster and open the Extensions pane.
    ///
    /// The query is asked on every open rather than cached from attach: a
    /// package can be installed, switched off or fail while this client is
    /// attached, and the reader opened the pane to see what is in force now.
    /// The rows are the host's own status roster, so the package a terminal
    /// names, its state and the tools it added are the ones the desktop's
    /// settings list shows. A filter narrows the rows without asking the Host
    /// again, because the roster already carries every field a search reads.
    fn extensions_command(&mut self, argument: Option<&str>) {
        let filter = argument
            .map(str::trim)
            .filter(|argument| !argument.is_empty());
        match self.client.extensions() {
            Ok(report) => {
                let summary = extensions_summary(&report, filter);
                self.settings = None;
                self.extensions = Some(ExtensionsPane {
                    report,
                    filter: filter.map(str::to_string),
                    scroll: 0,
                });
                // One pane at a time, like `/memory` and `/tasks`: the reader
                // asked for the roster, so whatever they had open yields.
                self.memory = None;
                self.files = None;
                self.diff = None;
                self.tasks = None;
                self.audit = None;
                self.detail = None;
                self.review = None;
                self.federation = None;
                self.trajectory = None;
                self.set_status(Level::Info, summary);
            }
            Err(error) => self.client_error(error),
        }
    }

    /// Leave the Extensions pane and keep whatever the composer holds.
    fn close_extensions(&mut self) {
        self.extensions = None;
        self.set_status(Level::Info, "extensions closed");
    }

    /// Walk the roster; the pane clamps the offset to its own height, so a
    /// short roster never scrolls away from its first row.
    fn scroll_extensions(&mut self, delta: isize) {
        if let Some(pane) = self.extensions.as_mut() {
            pane.scroll = pane.scroll.saturating_add_signed(delta);
        }
    }

    /// Read the open session's latest sealed review and open the Review pane.
    ///
    /// The store is the desktop's: a turn captures its changes at the tool
    /// boundary and seals them when it lands, so the reader sees exactly what
    /// the desktop's Review tab shows. Asking on every open rather than caching
    /// from attach is deliberate - a turn running beside this client seals its
    /// changeset at any moment, and the reader asked what it changed now.
    ///
    /// A change is named by the id the list printed or by the path the reader
    /// typed; the Host resolves both against the same changeset, so a diff is
    /// never built from a change another turn owns.
    fn review_command(&mut self, argument: Option<&str>) {
        let Some(session) = self.session.as_ref().map(|session| session.session_id) else {
            self.set_status(
                Level::Error,
                "no session is open to review · Ctrl-T creates one",
            );
            return;
        };
        let change = argument
            .map(str::trim)
            .filter(|argument| !argument.is_empty());
        match self.client.review(session, change) {
            Ok(report) => {
                let summary = review_summary(&report, change);
                self.settings = None;
                self.review = Some(ReviewPane {
                    report,
                    selection: 0,
                    scroll: 0,
                });
                // One pane at a time, like `/extensions` and `/files`: the
                // reader asked what a turn changed, so whatever they had open
                // yields rather than hiding the answer.
                self.memory = None;
                self.files = None;
                self.diff = None;
                self.tasks = None;
                self.audit = None;
                self.extensions = None;
                self.detail = None;
                self.federation = None;
                self.trajectory = None;
                self.set_status(Level::Info, summary);
            }
            Err(error) => self.client_error(error),
        }
    }

    /// Step out of a diff, or out of the pane.
    ///
    /// A diff is one level down from the list, so the first Escape goes back
    /// to the list - the desktop's breadcrumb - and the second closes the pane.
    /// Going back asks the Host again rather than keeping the previous report:
    /// the changeset may have been sealed by a turn that landed while the diff
    /// was open.
    fn back_review(&mut self) {
        let showing_diff = self
            .review
            .as_ref()
            .is_some_and(|pane| pane.report.diff.is_some());
        if showing_diff {
            self.review_command(None);
            return;
        }
        self.review = None;
        self.federation = None;
        self.set_status(Level::Info, "review closed");
    }

    /// Walk the change list, or scroll a diff.
    ///
    /// The pane clamps both: a selection never points past the last row of a
    /// changeset that shrank under it, and a short diff never scrolls away
    /// from its header.
    fn move_review(&mut self, delta: isize) {
        let Some(pane) = self.review.as_mut() else {
            return;
        };
        if pane.report.diff.is_some() {
            pane.scroll = pane.scroll.saturating_add_signed(delta);
            return;
        }
        let len = pane.report.changes.len();
        if len > 0 {
            pane.selection = pane.selection.saturating_add_signed(delta).min(len - 1);
        }
    }

    /// Open the highlighted change's diff, or step back from a diff.
    ///
    /// A diff has nothing to open, so Enter there is the same gesture as the
    /// breadcrumb: it returns to the list the reader came from.
    fn open_review_selection(&mut self) {
        let Some(pane) = self.review.as_ref() else {
            return;
        };
        if pane.report.diff.is_some() {
            self.review_command(None);
            return;
        }
        let len = pane.report.changes.len();
        let Some(change) = pane
            .report
            .changes
            .get(pane.selection.min(len.saturating_sub(1)))
            .map(|change| change.id.clone())
        else {
            self.set_status(Level::Info, "this changeset holds nothing to open");
            return;
        };
        self.review_command(Some(&change));
    }

    /// Open, close or scroll one tool call's detail pane.
    ///
    /// The pane is the terminal's form of the desktop's tool-output views: the
    /// transcript keeps one line per call, and a reader who needs the arguments
    /// or the whole output had no way to ask for them. The calls are addressed
    /// one-based, the same numbering the session list uses, and the index is
    /// always resolved against the projection this client holds - so a stale
    /// number is refused with the count instead of opening nothing.
    fn detail_command(&mut self, argument: Option<&str>) {
        let Some(session) = self.session.as_ref() else {
            self.set_status(Level::Error, "no session to read a tool call from");
            return;
        };
        let total = session.tool_calls.len();
        let argument = argument.map(str::trim).unwrap_or_default();
        if argument.eq_ignore_ascii_case("close") {
            if self.detail.is_none() {
                self.set_status(Level::Info, "the transcript is already showing");
                return;
            }
            self.close_detail();
            self.set_status(Level::Info, "detail closed");
            return;
        }
        if total == 0 {
            self.set_status(Level::Error, "this session has made no tool calls");
            return;
        }
        let index = match argument {
            "" => total - 1,
            number => match number.parse::<usize>() {
                Ok(0) | Err(_) => {
                    self.set_status(
                        Level::Error,
                        format!("usage: /detail [number|close]; this session made {total} call(s)"),
                    );
                    return;
                }
                Ok(number) if number > total => {
                    self.set_status(
                        Level::Error,
                        format!("call {number} does not exist; this session made {total}"),
                    );
                    return;
                }
                Ok(number) => number - 1,
            },
        };
        // One pane at a time: the reader asked for a call, so the memory pane
        // they had open yields rather than hiding the call.
        self.open_detail(index);
    }

    /// Show one call's detail, closing any pane that was in the way.
    ///
    /// `/detail` and a click on the call's transcript row reach the same pane,
    /// so the close, the scroll reset and the status line are written once.
    fn open_detail(&mut self, index: usize) {
        let total = self
            .session
            .as_ref()
            .map_or(0, |session| session.tool_calls.len());
        self.memory = None;
        self.audit = None;
        self.tasks = None;
        self.extensions = None;
        self.review = None;
        self.federation = None;
        self.trajectory = None;
        self.settings = None;
        self.files = None;
        self.diff = None;
        self.detail = Some(index);
        self.detail_scroll = 0;
        self.set_status(
            Level::Info,
            format!(
                "detail {}/{total} · Up/Down scrolls · Esc closes",
                index + 1
            ),
        );
    }

    /// Leave the detail pane and keep whatever the composer holds.
    fn close_detail(&mut self) {
        self.detail = None;
        self.detail_scroll = 0;
    }

    /// Open or search the Memory pane: the terminal's view of the durable
    /// store the desktop's Memory pane browses.
    ///
    /// The report is asked for on every call rather than cached from attach: a
    /// turn commits memories while this client is attached, and the reader
    /// opened the pane to see what the workspace remembers now. A search runs
    /// the Host's hybrid recall - the same ranking a turn's context assembly
    /// asks for - so the rows a reader finds are the rows a turn would use.
    fn memory_command(&mut self, argument: Option<&str>) {
        let query = argument
            .map(str::trim)
            .filter(|argument| !argument.is_empty());
        match self.client.memories(query) {
            Ok(report) => {
                let summary = memory_summary(&report, query);
                self.settings = None;
                self.memory = Some(MemoryPane {
                    report,
                    query: query.map(str::to_string),
                    scroll: 0,
                });
                // One pane at a time: the reader asked for memory, so the call
                // detail they had open yields rather than hiding the rows.
                self.detail = None;
                self.audit = None;
                self.files = None;
                self.diff = None;
                self.tasks = None;
                self.extensions = None;
                self.review = None;
                self.federation = None;
                self.trajectory = None;
                self.set_status(Level::Info, summary);
            }
            Err(error) => self.client_error(error),
        }
    }

    /// Leave the Memory pane and keep whatever the composer holds.
    fn close_memory(&mut self) {
        self.memory = None;
        self.set_status(Level::Info, "memory closed");
    }

    /// Walk the Memory pane's rows; the pane itself clamps the offset to its
    /// own height, so a short report never scrolls away from its first row.
    fn scroll_memory(&mut self, delta: isize) {
        if let Some(pane) = self.memory.as_mut() {
            pane.scroll = pane.scroll.saturating_add_signed(delta);
        }
    }

    /// Open or narrow the Audit pane: the terminal's form of the desktop's
    /// AUDIT page.
    ///
    /// Nothing is asked of the Host: the page is derived from the projection
    /// this client already holds, which is what makes it agree with the
    /// transcript row for row. A filter is free text, so a reader can ask for
    /// one tool, one path or one category without a separate syntax.
    fn audit_command(&mut self, argument: Option<&str>) {
        let filter = argument
            .map(str::trim)
            .filter(|argument| !argument.is_empty());
        let summary = match self.session.as_ref() {
            Some(session) => audit_summary(session, filter),
            None => "no session open · Ctrl-T creates one".to_string(),
        };
        self.settings = None;
        self.audit = Some(AuditPane {
            filter: filter.map(str::to_string),
            scroll: 0,
        });
        // One pane at a time, like `/memory` and `/detail`.
        self.memory = None;
        self.files = None;
        self.diff = None;
        self.tasks = None;
        self.extensions = None;
        self.detail = None;
        self.review = None;
        self.federation = None;
        self.trajectory = None;
        self.set_status(Level::Info, summary);
    }

    /// Leave the Audit pane and keep whatever the composer holds.
    fn close_audit(&mut self) {
        self.audit = None;
        self.set_status(Level::Info, "audit closed");
    }

    /// Walk the Audit pane's rows; the pane itself clamps the offset to its own
    /// height, so a short audit never scrolls away from its first row.
    fn scroll_audit(&mut self, delta: isize) {
        if let Some(pane) = self.audit.as_mut() {
            pane.scroll = pane.scroll.saturating_add_signed(delta);
        }
    }

    /// Move the transcript by `delta` rows, newest-first.
    ///
    /// A `/find` bar marks where a search landed, so the reader taking the
    /// document into their own hands is them leaving it: the reveal is spent.
    fn scroll_transcript(&mut self, delta: isize) {
        self.scroll = self.scroll.saturating_add_signed(delta);
        self.focus = None;
    }

    /// Reveal the rows of this conversation that contain `text`.
    ///
    /// A conversation is a document a terminal walks a screen at a time, and no
    /// chord reaches a row that has scrolled past, so a reader who remembers a
    /// word but not where it is gets a list of the rows that carry it and lands
    /// on the row they pick.
    ///
    /// The document searched is the one the last frame painted, so a hit is a
    /// row of the screen. A `#12` argument names the numbered row a list row
    /// carries, which is the line the pick of that row commits: the pick and
    /// the typed number end in the same place.
    fn find_command(&mut self, argument: &str) {
        let text = argument.trim();
        if let Some(number) = text.strip_prefix('#').and_then(|row| row.parse().ok()) {
            self.reveal_row(number);
            return;
        }
        if text.is_empty() {
            self.set_status(Level::Error, "usage: /find <text>");
            return;
        }
        let hits = find_rows(&self.document.borrow(), text);
        if hits.is_empty() {
            self.picker = None;
            self.set_status(Level::Error, picker::Kind::Find.empty());
            return;
        }
        // One hit is the row the reader wanted, so it is revealed instead of
        // being offered as a list of one.
        if hits.len() == 1 {
            self.picker = None;
            self.reveal_row(hits[0].0);
            return;
        }
        let count = if hits.len() <= MAX_FIND_ROWS {
            format!("{} row(s)", hits.len())
        } else {
            format!("{MAX_FIND_ROWS} of {} rows", hits.len())
        };
        self.picker = Some(picker::Picker::new(
            picker::Kind::Find.title(),
            find_items(&hits),
        ));
        self.set_status(
            Level::Info,
            format!("{count} match '{text}' · Enter reveals · Esc closes"),
        );
    }

    /// Scroll the transcript to numbered row `number`, and paint it as a bar
    /// while it stays on screen.
    ///
    /// The scroll is measured against the frame this client painted last,
    /// because the body's height is what decides where a row lands; a row
    /// revealed into a window nobody sized would move on the next event.
    fn reveal_row(&mut self, number: usize) {
        let total = self.document.borrow().len();
        if number == 0 || number > total {
            self.set_status(
                Level::Error,
                format!("no row {number} in a transcript of {total} row(s)"),
            );
            return;
        }
        let (width, height) = self.frame.get();
        let rows = self.body_rows(width.max(40), height.max(8));
        let row = number - 1;
        self.scroll = reveal_scroll(total, row, rows);
        self.focus = Some(row);
        self.set_status(Level::Info, format!("row {number} of {total}"));
    }

    /// Read or write the session's feedback note: the terminal's `/feedback`.
    ///
    /// The note is durable Host state, so no argument prints what the projection
    /// already holds rather than what this client last sent, and a write asks the
    /// Host and then re-reads the session. The desktop clears a note by
    /// submitting an empty draft; a terminal cannot send an empty line, so
    /// `clear` is that verb here.
    fn feedback_command(&mut self, argument: Option<&str>) {
        let Some(session_id) = self.current_session() else {
            self.set_status(Level::Error, "no session to record feedback on");
            return;
        };
        let Some(note) = argument else {
            let current = self
                .session
                .as_ref()
                .and_then(|session| session.feedback.clone());
            let status = match current {
                Some(note) => format!("feedback: {note}"),
                None => "no feedback recorded · usage: /feedback <note>; /feedback clear drops one"
                    .to_string(),
            };
            self.set_status(Level::Info, status);
            return;
        };
        let text = if clears_feedback(note) { "" } else { note };
        match self.client.set_feedback(session_id, text) {
            Ok(_) => {
                let status = if clears_feedback(note) {
                    "feedback cleared"
                } else {
                    "feedback saved"
                };
                self.set_status(Level::Info, status);
                self.refresh_all();
            }
            Err(error) => self.client_error(error),
        }
    }

    /// What is waiting for the running turn, or the `/queue clear` that drops
    /// it: the terminal's read-out of the desktop's queue dock.
    fn queue_command(&mut self, argument: Option<&str>) {
        match argument {
            None => {
                if self.queue.is_empty() {
                    self.set_status(Level::Info, "nothing is queued for this turn");
                    return;
                }
                let session = self
                    .queue
                    .session()
                    .map(|session_id| short(&session_id.to_string()))
                    .unwrap_or_else(|| "none".to_string());
                let rows = self
                    .queue
                    .lines()
                    .iter()
                    .enumerate()
                    .map(|(index, line)| format!("{}. {}", index + 1, queue_preview(line)))
                    .collect::<Vec<_>>()
                    .join(" · ");
                self.set_status(
                    Level::Info,
                    format!(
                        "queued {} line(s) for session {session}: {rows}",
                        self.queue.len()
                    ),
                );
            }
            Some("clear") => {
                let dropped = self.queue.len();
                self.queue.clear();
                self.set_status(
                    Level::Info,
                    format!("queue cleared: {dropped} line(s) dropped"),
                );
            }
            Some(other) => self.set_status(
                Level::Error,
                format!("usage: /queue · /queue clear (not '{other}')"),
            ),
        }
    }

    /// List the workspace's sessions, or open one by number or by a filter.
    ///
    /// The desktop's session menu is a picker with a search box; the terminal
    /// has no dialog, so the same rows ride on the status row and the number in
    /// front of a row is what opens it. A non-numeric argument filters the
    /// rows over the title and the newest message, the two fields the desktop
    /// searches, so `/sessions release` narrows the list the way typing in its
    /// search box does.
    fn sessions_command(&mut self, argument: Option<&str>) {
        if let Err(error) = self.refresh() {
            self.client_error(error);
            return;
        }
        let now = Utc::now();
        let Some(argument) = argument.map(str::trim).filter(|text| !text.is_empty()) else {
            // The desktop answers its session menu with a list, so a bare
            // `/sessions` opens one. Enter sends `/sessions <number>`, so a
            // pick and a typed number make the same Host call, and a workspace
            // with nothing to open keeps the sentence instead of an empty box.
            let rows = self.session_rows(None, now);
            self.open_picker(picker::Kind::Session);
            if self.picker.is_none() {
                let status = session_summary(&rows, self.sessions.len(), None);
                self.set_status(Level::Error, status);
            }
            return;
        };
        // The desktop's session menu is one popup whose rows pin, archive and
        // unarchive; a terminal names the row's action and the number the
        // listing printed, so `/sessions pin 2` is the same click as its Pin
        // row. A verb is only a verb when it carries that number: `/sessions
        // pin release` searches for a chat whose title says pin, which is what
        // the desktop's search box would do, and neither reading is guessed.
        let mut words = argument.split_whitespace();
        let head = words.next().unwrap_or_default();
        let number = words.next().and_then(|word| word.parse::<usize>().ok());
        match (head, number) {
            // The Archived filter has its own search box in the desktop
            // sidebar, so its rows take a filter too.
            ("archived", _) => {
                let filter = argument.strip_prefix(head).unwrap_or_default().trim();
                self.archived_command(filter, now);
                return;
            }
            ("pin" | "unpin" | "archive" | "unarchive", Some(number)) => {
                self.session_flag_command(head, number);
                return;
            }
            _ => {}
        }
        // A number opens a row. Rows keep their number in the whole list, so
        // the same `/sessions 3` opens the third session whatever is filtered.
        if let Ok(number) = argument.parse::<usize>() {
            if number == 0 || number > self.sessions.len() {
                self.set_status(
                    Level::Error,
                    format!(
                        "no session {number}; /sessions lists {} in this workspace",
                        self.sessions.len()
                    ),
                );
                return;
            }
            self.select(number - 1);
            let title = self
                .session
                .as_ref()
                .map(|session| session.title.clone())
                .unwrap_or_default();
            self.set_status(Level::Info, format!("opened session {number}: {title}"));
            return;
        }
        let rows = self.session_rows(Some(argument), now);
        let status = session_summary(&rows, self.sessions.len(), Some(argument));
        let level = if rows.is_empty() {
            Level::Error
        } else {
            Level::Info
        };
        self.set_status(level, status);
    }

    /// The numbered rows `/sessions` prints, narrowed when a query is given.
    fn session_rows(&self, query: Option<&str>, now: DateTime<Utc>) -> Vec<String> {
        self.sessions
            .iter()
            .enumerate()
            .filter(|(_, session)| query.is_none_or(|query| session_matches(session, query)))
            .map(|(index, session)| session_row(index, session, index == self.selected, now))
            .collect()
    }

    /// The session menu's flag rows, as a terminal spells them.
    ///
    /// The four verbs act on one number from the list they are printed
    /// against. Pin and archive name the state they want, not a toggle, so a
    /// repeated `/sessions pin 2` is the Host's refusal rather than an
    /// accidental unpin.
    fn session_flag_command(&mut self, verb: &str, number: usize) {
        // Unarchiving names a row of the Archived filter, because that is the
        // listing that printed the number; the other three name a row of the
        // session list. Reading the wrong list would act on a chat the reader
        // never pointed at.
        let target = if verb == "unarchive" {
            // The filter is re-read first so the number is checked against the
            // rows the Host holds now, the way `/restore` re-reads the bin.
            if let Err(error) = self.refresh_archived() {
                self.client_error(error);
                return;
            }
            if number == 0 || number > self.archived.len() {
                self.set_status(
                    Level::Error,
                    format!(
                        "no archived session {number}; /sessions archived lists {} of them",
                        self.archived.len()
                    ),
                );
                return;
            }
            self.archived[number - 1].clone()
        } else {
            if number == 0 || number > self.sessions.len() {
                self.set_status(
                    Level::Error,
                    format!(
                        "no session {number}; /sessions lists {} in this workspace",
                        self.sessions.len()
                    ),
                );
                return;
            }
            self.sessions[number - 1].clone()
        };
        let answer = match verb {
            "pin" => self.client.pin_session(target.session_id, true),
            "unpin" => self.client.pin_session(target.session_id, false),
            "archive" => self.client.archive_session(target.session_id, true),
            _ => self.client.archive_session(target.session_id, false),
        };
        match answer {
            Ok(_) => {
                self.set_status(
                    Level::Info,
                    match verb {
                        "pin" => format!("pinned session {number}: {}", target.title),
                        "unpin" => format!("unpinned session {number}: {}", target.title),
                        "archive" => format!(
                            "archived session {number}: {} · /sessions archived lists it",
                            target.title
                        ),
                        _ => format!("unarchived session {number}: {}", target.title),
                    },
                );
                self.refresh_all();
                // The archived list the reader may be holding has a new
                // membership, so it is re-read instead of edited here.
                if matches!(verb, "archive" | "unarchive")
                    && let Err(error) = self.refresh_archived()
                {
                    self.client_error(error);
                }
            }
            // A refusal is the Host's: a turn in flight is the one reason
            // archiving is unsafe, and its words are the answer.
            Err(error) => {
                self.client_error(error);
                self.refresh_all();
            }
        }
    }

    /// Re-read the Archived filter from the Host.
    ///
    /// The list is asked for on every use rather than kept from the last
    /// listing: another client, or this client's own turn, can archive or
    /// unarchive a chat between one command and the next.
    fn refresh_archived(&mut self) -> Result<(), ClientError> {
        self.archived = self.client.archived()?;
        Ok(())
    }

    /// The numbered rows `/sessions archived` prints, in the order
    /// `/sessions unarchive` addresses them.
    fn archived_rows(&self, filter: Option<&str>, now: DateTime<Utc>) -> Vec<String> {
        self.archived
            .iter()
            .enumerate()
            .filter(|(_, session)| filter.is_none_or(|filter| session_matches(session, filter)))
            .map(|(index, session)| session_row(index, session, false, now))
            .collect()
    }

    /// List the Archived filter, narrowed by a search when one is given.
    ///
    /// The rows keep the number they hold in the whole filter, so
    /// `/sessions unarchive 4` still names the fourth archived chat while a
    /// search narrows what is on screen - the same rule `/sessions` follows.
    fn archived_command(&mut self, filter: &str, now: DateTime<Utc>) {
        if let Err(error) = self.refresh_archived() {
            self.client_error(error);
            return;
        }
        let query = Some(filter).filter(|filter| !filter.is_empty());
        if query.is_none() {
            // A bare `/sessions archived` opens the list the desktop draws in
            // its sidebar; Enter there is the Unarchive row of that list's own
            // menu, so a pick and `/sessions unarchive <n>` make one call.
            self.open_picker(picker::Kind::Archived);
            if self.picker.is_some() {
                return;
            }
        }
        let rows = self.archived_rows(query, now);
        let status = archived_summary(&rows, self.archived.len(), query);
        let level = if rows.is_empty() {
            Level::Error
        } else {
            Level::Info
        };
        self.set_status(level, status);
    }

    /// Write the open session as Markdown, the terminal's `/export`.
    ///
    /// The desktop's Export row saves a ZIP through a native file dialog; a
    /// terminal has no dialog, so the path is the command's argument and a
    /// bare `/export` writes under the Host's data directory where the next
    /// export, or the desktop, can find the conversation.
    ///
    /// Nothing about this is durable Host state: the file is built from the
    /// projection this client already holds, so an export asks the Host for
    /// nothing and changes nothing a second client would see.
    fn export(&mut self, path: Option<String>) {
        let Some(session) = self.session.clone() else {
            self.set_status(Level::Error, "no session to export; Ctrl-T creates one");
            return;
        };
        let named = export::file_name(&session.title);
        let target = match path.as_deref() {
            Some(path) => export::resolve(path, self.workspace.as_deref(), &session.title),
            // The data directory holds the exports a terminal wrote without
            // naming a place, so they sit beside the sessions they came from.
            None => self.data_dir.join("exports").join(named),
        };
        match export::write(&target, &export::markdown(&session)) {
            Ok(()) => self.set_status(Level::Info, format!("exported to {}", target.display())),
            Err(error) => self.set_status(
                Level::Error,
                format!("could not write {}: {error}", target.display()),
            ),
        }
    }

    /// Put one answer on the reader's clipboard, the terminal's `/copy`.
    ///
    /// A terminal client cannot reach a native clipboard, so the copy leaves
    /// through OSC 52 ([`term::copy`]): the emulator in front of the reader
    /// sets its selection, which is what makes the gesture work in the same
    /// places the terminal does - over SSH, in tmux, behind a screen. The text
    /// is the projection's, not the frame's: a wrapped row, an answer scrolled
    /// out of view and the markers a row carries are not part of the message,
    /// so what lands on the clipboard is what the model wrote.
    fn copy_message(&mut self, from_end: usize) {
        let Some(session) = self.session.as_ref() else {
            self.set_status(Level::Error, "no session to copy from; Ctrl-T creates one");
            return;
        };
        // The message is taken out of the projection before the status row is
        // written: the copy is owned text, so the frame and the Host are not
        // borrowed while the terminal is written to.
        let Some(text) = copy_text(session, from_end).map(str::to_string) else {
            let missing = match from_end {
                1 => "no answer in this session to copy".to_string(),
                count => format!("this session has no answer {count} back to copy"),
            };
            self.set_status(Level::Error, missing);
            return;
        };
        match term::copy(&text) {
            Ok(copied) if copied.cut => self.set_status(
                Level::Info,
                format!(
                    "copied the first {} characters; the answer is longer than one clipboard escape carries",
                    copied.chars
                ),
            ),
            Ok(copied) => self.set_status(
                Level::Info,
                format!("copied {} characters to the clipboard", copied.chars),
            ),
            Err(error) => self.set_status(Level::Error, format!("could not copy: {error}")),
        }
    }

    /// Put the open session's id on the reader's clipboard.
    ///
    /// The desktop's session menu carries the same row. The id comes from the
    /// projection - the Host's durable fact - not from the rail's label, so
    /// what lands on the clipboard is the string another client's
    /// `/sessions <number>` or a journal lookup would accept.
    fn copy_session_id(&mut self) {
        let Some(id) = self.current_session() else {
            self.set_status(Level::Error, "no session to copy from; Ctrl-T creates one");
            return;
        };
        let text = id.to_string();
        match term::copy(&text) {
            Ok(_) => self.set_status(Level::Info, format!("copied session id {text}")),
            Err(error) => self.set_status(Level::Error, format!("could not copy: {error}")),
        }
    }

    /// Rename the open session through the Host.
    ///
    /// The title is a durable Host fact, so the terminal never renames one
    /// locally: the projection is re-read, and the reader sees the name land
    /// the same way a second client would.
    fn rename(&mut self, title: String) {
        let Some(session_id) = self.current_session() else {
            self.set_status(Level::Error, "no session to rename; Ctrl-T creates one");
            return;
        };
        match self.client.rename_session(session_id, title.clone()) {
            Ok(_) => {
                self.refresh_all();
                let done = self
                    .session
                    .as_ref()
                    .is_some_and(|session| session.title == title);
                self.set_status(
                    Level::Info,
                    if done {
                        format!("session renamed to '{title}'")
                    } else {
                        "the Host accepted the rename but the projection still shows the old title"
                            .to_string()
                    },
                );
            }
            Err(error) => {
                self.client_error(error);
                self.refresh_all();
            }
        }
    }

    /// Ask the Host to fold this session's older history.
    ///
    /// The summary is written by the Host's executor, so this only reports
    /// that the request landed: the checkpoint arrives as an event, the same
    /// way a turn's answer does.
    fn compact(&mut self) {
        let Some(session_id) = self.current_session() else {
            self.set_status(Level::Error, "no session to compact; Ctrl-T creates one");
            return;
        };
        match self.client.compact_session(session_id) {
            Ok(_) => self.set_status(
                Level::Info,
                "compaction requested; the Host is folding this session's history",
            ),
            Err(error) => self.client_error(error),
        }
    }

    /// Report what this session is pursuing, as the Host last journalled it.
    ///
    /// The goal is durable Host state, so the terminal reads it from the
    /// projection rather than remembering what `/goal` sent a moment earlier.
    fn show_goal(&mut self) {
        match self
            .session
            .as_ref()
            .and_then(|session| session.goal.as_ref())
        {
            None => self.set_status(
                Level::Info,
                "no goal · usage: /goal <objective>; /goal clear drops one",
            ),
            Some(goal) => {
                let state = format!("{:?}", goal.state).to_lowercase();
                let blocked = goal
                    .blocked_reason
                    .as_deref()
                    .map(|reason| format!(" · blocked: {reason}"))
                    .unwrap_or_default();
                self.set_status(
                    Level::Info,
                    format!(
                        "goal {state} · round {}/{} · {}{blocked}",
                        goal.rounds, goal.max_rounds, goal.objective
                    ),
                );
            }
        }
    }

    /// Run one goal verb through the Host and report the projection it left.
    ///
    /// Every verb travels as a whole `GoalChanged` fact, so the terminal re-reads
    /// the session instead of remembering what it sent: a verb landed when the
    /// projection says so, not when the command came back accepted.
    fn commit_goal(
        &mut self,
        verb: &str,
        command: impl FnOnce(SessionId) -> Result<CommandOutcome, ClientError>,
        landed: impl FnOnce(&SessionProjection) -> bool,
    ) {
        let Some(session_id) = self.current_session() else {
            self.set_status(
                Level::Error,
                format!("no session to {verb}; Ctrl-T creates one"),
            );
            return;
        };
        match command(session_id) {
            Ok(_) => {
                self.refresh_all();
                let done = self.session.as_ref().is_some_and(landed);
                self.set_status(
                    Level::Info,
                    if done {
                        format!("goal {verb}")
                    } else {
                        format!("the Host accepted '{verb}' but the projection does not show it")
                    },
                );
            }
            Err(error) => {
                self.client_error(error);
                self.refresh_all();
            }
        }
    }

    /// Replace the open session's goal with one statement.
    ///
    /// A goal set here lands in the Host, so a second client sees it the same
    /// way the desktop's goal panel does; the terminal only reports what the
    /// projection shows.
    fn set_goal(&mut self, objective: String) {
        let client = self.client.clone();
        let expected = objective.clone();
        self.commit_goal(
            "set",
            move |session_id| client.set_goal(session_id, objective),
            move |session| {
                session
                    .goal
                    .as_ref()
                    .is_some_and(|goal| goal.objective == expected)
            },
        );
    }

    /// Rewrite the open session's goal, keeping its state and round count.
    fn edit_goal(&mut self, objective: String) {
        let client = self.client.clone();
        let expected = objective.clone();
        self.commit_goal(
            "edited",
            move |session_id| client.edit_goal(session_id, objective),
            move |session| {
                session
                    .goal
                    .as_ref()
                    .is_some_and(|goal| goal.objective == expected)
            },
        );
    }

    /// Stop the open session's goal, keeping its objective and rounds.
    fn pause_goal(&mut self) {
        let client = self.client.clone();
        self.commit_goal(
            "paused",
            move |session_id| client.pause_goal(session_id),
            |session| {
                session
                    .goal
                    .as_ref()
                    .is_some_and(|goal| goal.state == GoalState::Paused)
            },
        );
    }

    /// Arm a paused or blocked goal again.
    fn resume_goal(&mut self) {
        let client = self.client.clone();
        self.commit_goal(
            "resumed",
            move |session_id| client.resume_goal(session_id),
            |session| {
                session
                    .goal
                    .as_ref()
                    .is_some_and(|goal| goal.state == GoalState::Active)
            },
        );
    }

    /// Drop the open session's goal through the Host.
    fn clear_goal(&mut self) {
        let client = self.client.clone();
        self.commit_goal(
            "cleared",
            move |session_id| client.clear_goal(session_id),
            |session| session.goal.is_none(),
        );
    }

    /// Send one patch and report the Host's verdict.
    ///
    /// The projection is re-read either way: a rejected command changes
    fn configure(
        &mut self,
        change: SessionConfigChange,
        applied: impl FnOnce(&SessionConfig) -> bool,
    ) {
        let Some(session_id) = self.current_session() else {
            self.set_status(Level::Error, "no session to configure; Ctrl-T creates one");
            return;
        };
        match self.client.configure_session(session_id, change) {
            Ok(_) => {
                self.refresh_all();
                let done = self
                    .session
                    .as_ref()
                    .map(|session| applied(&session.config))
                    .unwrap_or(false);
                self.set_status(
                    Level::Info,
                    if done {
                        "session configuration updated"
                    } else {
                        "the Host accepted the change but the projection does not show it"
                    },
                );
            }
            Err(error) => {
                self.client_error(error);
                self.refresh_all();
            }
        }
    }

    /// Insert a line break at the cursor, leaving the cursor after it.
    fn newline(&mut self) {
        self.input.insert(self.cursor, '\n');
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if let Some((index, _)) = self.input[..self.cursor].char_indices().next_back() {
            self.input.replace_range(index..self.cursor, "");
            self.cursor = index;
        }
    }

    /// Move one entry through the prompt history, cursor at the end of the
    /// recalled line. A step past either end leaves the prompt as it is.
    fn recall(&mut self, older: bool) {
        let line = if older {
            self.history.older(&self.input)
        } else {
            self.history.newer()
        };
        if let Some(line) = line {
            self.cursor = line.len();
            self.input = line;
        }
    }

    /// Flip plan mode through the Host, the terminal's form of the desktop's
    /// plan switch and opencode's own `Tab` binding.
    fn toggle_plan(&mut self) {
        let current = self
            .session
            .as_ref()
            .is_some_and(|session| session.config.plan_mode);
        self.configure(
            SessionConfigChange {
                plan_mode: Some(!current),
                ..SessionConfigChange::default()
            },
            |config| config.plan_mode == !current,
        );
    }

    /// A title is free text the reader typed, like `/rename`'s; no title names
    /// the session after the clock, which is what `Ctrl-T` and bare `/new` do.
    /// Either way the row is what the Host created: the client refreshes and
    /// opens the session it was handed rather than inventing one locally.
    fn create_session(&mut self, title: Option<String>) {
        let title = session_title(title.as_deref());
        match self.client.create_session(title) {
            Ok(session_id) => {
                self.refresh_all();
                if let Some(index) = self
                    .sessions
                    .iter()
                    .position(|session| session.session_id == session_id)
                {
                    self.select(index);
                }
                self.set_status(Level::Info, "session created");
            }
            Err(error) => self.client_error(error),
        }
    }

    /// Branch the open session into a new chat.
    ///
    /// The fork is the Host's, so the client does nothing but ask for it and
    /// open the chat the Host names: the rows the fork opens with are read back
    /// from the projection the same way every other client reads them, rather
    /// than copied here where a reconnect would lose them.
    fn fork_session(&mut self) {
        let Some(session_id) = self.current_session() else {
            self.set_status(Level::Error, "no session to fork; Ctrl-T creates one");
            return;
        };
        match self.client.fork_session(session_id) {
            Ok(forked) => {
                self.refresh_all();
                if let Some(index) = self
                    .sessions
                    .iter()
                    .position(|session| session.session_id == forked)
                {
                    self.select(index);
                }
                self.set_status(
                    Level::Info,
                    "session forked; the branch opens with this transcript",
                );
            }
            Err(error) => self.client_error(error),
        }
    }

    fn cancel_execution(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let live = session.executions.iter().rev().find(|execution| {
            matches!(
                execution.status,
                ExecutionStatus::Queued
                    | ExecutionStatus::Running
                    | ExecutionStatus::WaitingApproval
            )
        });
        let Some(execution) = live else {
            self.set_status(Level::Info, "no running execution in this session");
            return;
        };
        match self
            .client
            .cancel_execution(session.session_id, execution.execution_id)
        {
            Ok(_) => {
                self.set_status(Level::Info, "cancellation accepted by the Host");
                self.refresh_all();
            }
            Err(error) => self.client_error(error),
        }
    }

    fn decide_approval(&mut self, approve: bool, note: Option<String>) {
        let Some(approval) = self.pending_approval().cloned() else {
            self.set_status(Level::Info, "no pending approval in this session");
            return;
        };
        // The note is the operator's reason, and the Host audits it beside the
        // verdict: a reader who typed one is told it went, not just that the
        // action stopped.
        let sent = note
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string);
        match self
            .client
            .decide_approval(approval.session_id, &approval, approve, sent.clone())
        {
            Ok(_) => {
                let verdict = if approve { "approved" } else { "rejected" };
                let message = match sent {
                    Some(note) => format!("approval {verdict} · note: {note}"),
                    None => format!("approval {verdict}"),
                };
                self.set_status(Level::Info, message);
                self.refresh_all();
            }
            Err(error) => self.client_error(error),
        }
    }

    /// Whether the open session still has a turn the Host has not finished.
    ///
    /// The projection is the only authority: a streamed delta can arrive after
    /// the commit that ended the turn, so a client that trusted the stream
    /// would queue behind a turn that no longer exists.
    fn busy(&self) -> bool {
        self.session.as_ref().is_some_and(|session| {
            session
                .executions
                .iter()
                .any(|execution| unfinished(execution.status))
        })
    }

    /// Send the head of the queue once its session is idle again.
    ///
    /// One line per call, and only while the queued session is the open one:
    /// this client holds one projection at a time, so it can prove a session
    /// idle only when it is looking at it. A line that fails to send goes back
    /// to the head of the queue instead of being lost.
    fn flush_queue(&mut self) {
        if self.flushing || self.queue.is_empty() {
            return;
        }
        let Some(session_id) = self.current_session() else {
            return;
        };
        if self.queue.session() != Some(session_id) || self.busy() {
            return;
        }
        let Some(text) = self.queue.pop() else {
            return;
        };
        self.flushing = true;
        let sent = self.client.send_message(session_id, text.clone());
        self.flushing = false;
        match sent {
            Ok(_) => {
                let status = match self.queue.len() {
                    0 => "queued turn sent".to_string(),
                    waiting => format!("queued turn sent; {waiting} still waiting"),
                };
                self.set_status(Level::Info, status);
                self.scroll = 0;
            }
            Err(error) => {
                // The Host refused it, so nothing was accepted: the line stays
                // at the head and the reader is told why it did not move.
                self.queue.requeue(text);
                self.client_error(error);
            }
        }
    }

    fn refresh_all(&mut self) {
        if let Err(error) = self.refresh() {
            self.client_error(error);
        }
    }

    /// Re-read the projections. A query is a snapshot of committed state, so
    /// this never shows a half-applied command.
    fn refresh(&mut self) -> Result<(), ClientError> {
        self.sessions = self.client.sessions()?;
        self.host = Some(self.client.status()?);
        if self.sessions.is_empty() {
            self.selected = 0;
            self.session = None;
        } else {
            self.selected = self.selected.min(self.sessions.len() - 1);
            self.refresh_session();
        }
        Ok(())
    }

    fn refresh_session(&mut self) {
        let Some(session_id) = self.current_session() else {
            self.session = None;
            self.live = None;
            return;
        };
        match self.client.session(session_id) {
            Ok(session) => self.session = session,
            Err(error) => self.client_error(error),
        }
        // The projection is the only truth about which calls exist, so a pane
        // that outlived its call (a resumed session, a cleared transcript) is
        // closed rather than left pointing at a row that is gone.
        if let Some(index) = self.detail
            && self
                .session
                .as_ref()
                .is_none_or(|session| index >= session.tool_calls.len())
        {
            self.close_detail();
        }
        self.reconcile_live();
        // A turn that asked a question via `ask_user` leaves a card the reader
        // answers next; the projection carries the call, so every refresh looks
        // for one rather than trusting a stream event to have reported it.
        self.sync_question();
    }

    /// Drop streamed text once the Host has committed that turn.
    ///
    /// The projection, not the stream, decides when a turn stopped running: a
    /// delta can arrive after the commit that already carries the full answer.
    fn reconcile_live(&mut self) {
        let Some(live) = &self.live else {
            return;
        };
        let settled = self.session.as_ref().is_none_or(|session| {
            session.session_id != live.session
                || !session.executions.iter().any(|execution| {
                    execution.execution_id == live.execution && unfinished(execution.status)
                })
        });
        if settled {
            self.live = None;
        }
    }

    /// Subscribe from the last applied event, reconciling when the Host no
    /// longer retains that position.
    fn subscribe(&mut self) -> Result<(), ClientError> {
        loop {
            let response = self.client.subscribe(Some(Cursor {
                sequence: self.applied,
            }))?;
            match response {
                Response::Subscribed(subscribed) => {
                    self.subscription = Some(subscribed.subscription_id);
                    for event in subscribed.replay {
                        self.applied = event.sequence;
                    }
                    return Ok(());
                }
                Response::CursorExpired(expired) => {
                    // The Host pruned the replay window: adopt its head and
                    // re-read the projections instead of inventing state.
                    self.applied = expired.head;
                    self.refresh()?;
                    self.set_status(
                        Level::Info,
                        "the Host's replay window moved; state reconciled",
                    );
                }
                other => {
                    return Err(ClientError::Contract(format!(
                        "subscribe answered with {other:?}"
                    )));
                }
            }
        }
    }

    fn set_status(&mut self, level: Level, message: impl Into<String>) {
        self.status = Some((level, message.into()));
    }

    fn client_error(&mut self, error: ClientError) {
        if error.is_disconnected() {
            self.quit = true;
        }
        self.set_status(Level::Error, error.to_string());
    }
}

/// One row of the transcript timeline.
#[derive(Clone, Copy)]
enum Entry<'a> {
    Message(&'a MessageView),
    Tool(&'a ToolCallView),
}

impl Entry<'_> {
    fn at(&self) -> DateTime<Utc> {
        match self {
            Self::Message(message) => message.at,
            Self::Tool(call) => call.started_at,
        }
    }
}

/// One client-side command typed into the prompt.
///
/// A command is the terminal's form of a desktop picker: it changes the
/// session's configuration through the Host and never locally, so a second
/// client sees the same chips a moment later.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SlashCommand {
    Help,
    Model(Option<String>, Option<String>),
    Preset(Option<String>),
    Permission(Option<String>),
    Plan(Option<bool>),
    /// Fold the session's older history into one checkpoint.
    Compact,
    /// Put one of the model's answers on the reader's clipboard; no number
    /// copies the newest, and `n` counts answers back from it.
    Copy(Option<usize>),
    /// Put the open session's id on the reader's clipboard.
    ///
    /// The desktop's session menu has the same row: an id is what a reader
    /// pastes into a bug report or a journal lookup, and a terminal's own
    /// selection cannot always reach a row the frame cut.
    CopyId,
    /// What the session is pursuing, read from the projection.
    GoalShow,
    /// Replace the session's goal with one statement, taken verbatim.
    GoalSet(String),
    /// Rewrite an existing goal, keeping its state and round count.
    GoalEdit(String),
    /// Stop the goal without dropping it.
    GoalPause,
    /// Arm a paused or blocked goal again.
    GoalResume,
    /// Drop the session's goal.
    GoalClear,
    /// Rename the open session; no title asks for the current one.
    Rename(Option<String>),
    /// Save the open session as Markdown; no path writes under the data dir.
    Export(Option<String>),
    /// Write the prompt in the editor the reader already uses, and take the
    /// buffer back as the prompt.
    ///
    /// A terminal's composer is the few rows between the transcript and the
    /// footer, and a long prompt written there scrolls away from its own
    /// beginning. `^X` is the same gesture, and the chord opencode and every
    /// shell bind for it.
    Editor,
    /// Create a session; no title names one after the clock.
    New(Option<String>),
    /// Branch the open session: a new chat that opens with its transcript.
    ///
    /// The desktop session menu's fork row, reached from the prompt: the fork
    /// is a Host fact, so the branch a terminal makes is one the desktop sees.
    Fork,
    /// List the workspace's sessions; a number opens one, text filters them.
    Sessions(Option<String>),
    /// The skills this build registered; text searches names and descriptions.
    Skills(Option<String>),
    /// The lines waiting for the running turn; `clear` drops them.
    Queue(Option<String>),
    /// The session's feedback note; no argument reads it, `clear` drops it.
    Feedback(Option<String>),
    /// Settle the pending approval in favour of the action.
    ///
    /// `^Y` is the chord a reader presses while a turn waits; the command names
    /// the same verdict for the reader who is looking at `/help` instead of the
    /// footer, and for the one whose terminal never delivers the chord.
    Approve,
    /// Refuse the pending approval, with the reader's reason when they gave one.
    ///
    /// The desktop dialog's deny box writes a note the Host audits beside the
    /// verdict. A terminal has no box to open, so the rest of the line is that
    /// note: `/deny the target is production` refuses and says why.
    Deny(Option<String>),
    /// Open one tool call's arguments and full output; no argument opens the
    /// newest call, `close` returns to the transcript.
    Detail(Option<String>),
    /// The token buckets this session's turns reported.
    UsageShow,
    /// The MCP servers this Host connected; a name lists one server's tools.
    Mcp(Option<String>),
    /// The workspace's durable memories; text runs the hybrid recall.
    Memory(Option<String>),
    /// The session's audited tool calls; text filters the list.
    Audit(Option<String>),
    /// Reveal the rows of this conversation that contain the text.
    ///
    /// A long conversation is a document a terminal can only walk a screen at
    /// a time, so the reader who remembers a word but not where it is gets a
    /// list of the rows that carry it and lands on the one they pick. It is
    /// client-side: the rows are the transcript the projection already gave
    /// this client, so nothing is asked of the Host to search it.
    Find(String),
    /// The workspace the Host serves; a path lists that directory or reads
    /// that file, and no path opens the root.
    Files(Option<String>),
    /// The working copy's changes, or one path's comparison: the desktop's
    /// Diff tab. No argument lists every changed path, and a path opens that
    /// file's unified patch.
    Diff(Option<String>),
    /// The workspace's durable task board: the desktop Tasks card's sections.
    ///
    /// One id opens that task's record the way expanding its card does; the
    /// board itself is the default, because a reader who typed the command
    /// without a name asked what is on the board.
    Tasks(Option<String>),
    /// The extensions this build installed; text searches ids and names.
    Extensions(Option<String>),
    /// The node this build runs, the peers it trusts and their grants.
    Federation,
    /// The browser's page: the desktop's Browser tab, as a terminal reads it.
    Browser,
    /// The delegated child runs this conversation started: the desktop's task
    /// graph children and header list. No argument reads the open session's
    /// lineage, `all` reads every run the Host holds, and a number opens one
    /// run's prompt and answer.
    Subagents(Option<String>),
    /// The open session's typed event log: every turn the Host ran with the
    /// messages and calls inside it.
    Trajectory,
    /// The session's latest sealed review; a change id or path shows its
    /// prepared diff, and no argument lists the changeset.
    Review(Option<String>),
    /// The workspace's bin: no argument lists the parked sessions, and a
    /// `/sessions` row number parks that session.
    Trash(Option<String>),
    /// Take one bin row back out, addressed by the number `/trash` printed.
    Restore(Option<String>),
    /// Drop one bin row for good, addressed by the number `/trash` printed.
    Purge(Option<String>),
    /// The client's own shell, in place of the transcript.
    Terminal,
    /// The model's unanswered `ask_user` question, brought back after Escape
    /// put it aside.
    Questions,
    /// What this Host's settings resolve to: no argument lists every section,
    /// and one name narrows the pane to that section.
    Settings(Option<String>),
    /// Write one settings field the desktop's dialog also writes: no argument
    /// opens the rail and counts the writable rows, one key clears a field that
    /// can be emptied, and a key with a value writes it.
    Set {
        /// The row key the Host reported; `None` lists the keys.
        key: Option<String>,
        /// The text form of the new value; `None` clears a clearable field.
        value: Option<String>,
    },
    /// A known command typed with the wrong arguments.
    Usage(&'static str),
    /// A slash line no command owns.
    Unknown(String),
}

impl SlashCommand {
    /// Whether this command swaps the body for a document.
    ///
    /// The body is one column, so a command that shows a pane replaces whatever
    /// pane is open instead of layering over it: the reader's newest request is
    /// what the screen answers. The list is kept beside [`App::close_panes`],
    /// which it triggers.
    fn opens_pane(&self) -> bool {
        matches!(
            self,
            Self::Settings(_)
                | Self::Set { .. }
                | Self::Files(_)
                | Self::Diff(_)
                | Self::Tasks(_)
                | Self::Extensions(_)
                | Self::Federation
                | Self::Browser
                | Self::Subagents(_)
                | Self::Trajectory
                | Self::Review(_)
                | Self::Memory(_)
                | Self::Audit(_)
                | Self::Detail(_)
                | Self::Terminal
        )
    }

    /// Parse one prompt line; `None` keeps it as a turn for the model.
    fn parse(text: &str) -> Option<Self> {
        let mut words = text.split_whitespace();
        let head = words.next()?;
        if !head.starts_with('/') {
            return None;
        }
        // A title is free text, not one word: the rest of the line is taken
        // verbatim so a rename can carry spaces.
        let rest = text[head.len()..].trim();
        match head {
            "/help" | "/?" => Some(Self::Help),
            "/model" => match (words.next(), words.next(), words.next()) {
                (None, None, None) => Some(Self::Model(None, None)),
                (Some(provider), Some(model), None) => Some(Self::Model(
                    Some(provider.to_string()),
                    Some(model.to_string()),
                )),
                _ => Some(Self::Usage(
                    "usage: /model <provider> <model>; /model alone opens the list",
                )),
            },
            "/preset" => match (words.next(), words.next()) {
                (None, None) => Some(Self::Preset(None)),
                (Some(preset), None) => Some(Self::Preset(Some(preset.to_string()))),
                _ => Some(Self::Usage(
                    "usage: /preset <id>; /preset alone opens the list",
                )),
            },
            "/permission" => match (words.next(), words.next()) {
                (None, None) => Some(Self::Permission(None)),
                (Some(mode), None) => Some(Self::Permission(Some(mode.to_string()))),
                _ => Some(Self::Usage(
                    "usage: /permission <mode>; /permission alone opens the list",
                )),
            },
            "/plan" => match (words.next(), words.next()) {
                (None, None) => Some(Self::Plan(None)),
                (Some("on"), None) => Some(Self::Plan(Some(true))),
                (Some("off"), None) => Some(Self::Plan(Some(false))),
                _ => Some(Self::Usage("usage: /plan [on|off]")),
            },
            "/compact" => match words.next() {
                None => Some(Self::Compact),
                _ => Some(Self::Usage("usage: /compact")),
            },
            // The branch is of the open session and the Host names it, so there
            // is nothing to spell here: a title would be a second decision the
            // desktop's fork row does not ask for either.
            "/fork" => match words.next() {
                None => Some(Self::Fork),
                _ => Some(Self::Usage("usage: /fork")),
            },
            // A count is one number: 0 would name a message before the newest
            // one, so it is refused here rather than answered with "no answer".
            // `id` names the session itself, which is the desktop session
            // menu's other clipboard row.
            "/copy" => match (words.next(), words.next()) {
                (None, None) => Some(Self::Copy(None)),
                (Some(word), None) if word.eq_ignore_ascii_case("id") => Some(Self::CopyId),
                (Some(count), None) => match count.parse::<usize>() {
                    Ok(count) if count > 0 => Some(Self::Copy(Some(count))),
                    _ => Some(Self::Usage(
                        "usage: /copy [number|id] — no number copies the newest answer, `id` the session",
                    )),
                },
                _ => Some(Self::Usage(
                    "usage: /copy [number|id] — no number copies the newest answer, `id` the session",
                )),
            },
            // The verdict the footer's chords spell out. Approving takes no
            // argument; refusing takes the reader's reason as free text, the
            // way `/feedback` takes a sentence.
            "/approve" => match rest {
                "" => Some(Self::Approve),
                _ => Some(Self::Usage("usage: /approve — `^Y` is the same verdict")),
            },
            "/deny" => match rest {
                "" => Some(Self::Deny(None)),
                note => Some(Self::Deny(Some(note.to_string()))),
            },
            "/usage" => match words.next() {
                None => Some(Self::UsageShow),
                _ => Some(Self::Usage("usage: /usage")),
            },
            // A server name is one word: it is the namespace its tools
            // register under, so spaces in the argument would name nothing.
            "/mcp" => match (words.next(), words.next()) {
                (None, None) => Some(Self::Mcp(None)),
                (Some(name), None) => Some(Self::Mcp(Some(name.to_string()))),
                _ => Some(Self::Usage("usage: /mcp [server]")),
            },
            // A search is free text, not a name: the rest of the line is the
            // recall query, and an empty one lists the store.
            "/memory" => match rest {
                "" => Some(Self::Memory(None)),
                query => Some(Self::Memory(Some(query.to_string()))),
            },
            // A search is free text and it is required: an empty `/find` would
            // list every row, which is the transcript the reader is already
            // looking at.
            "/find" => match rest {
                "" => Some(Self::Usage("usage: /find <text>")),
                text => Some(Self::Find(text.to_string())),
            },
            // A filter is free text for the same reason: it may carry a path
            // with spaces, and an empty one lists every audited call.
            "/audit" => match rest {
                "" => Some(Self::Audit(None)),
                filter => Some(Self::Audit(Some(filter.to_string()))),
            },
            // A path is free text, like `/export`'s, and it is resolved by the
            // Host: an empty one opens the workspace root the Host serves.
            "/files" => match rest {
                "" => Some(Self::Files(None)),
                path => Some(Self::Files(Some(path.to_string()))),
            },
            // A path is free text here for the same reason it is in `/files`: a
            // comparison may name a file with spaces in it, and an empty
            // argument lists the changes the desktop's Diff tab opens on.
            "/diff" => match rest {
                "" => Some(Self::Diff(None)),
                path => Some(Self::Diff(Some(path.to_string()))),
            },
            // The board is the workspace's, not one session's: a scheduled task
            // outlives the session that created it, so there is no argument.
            "/tasks" => match (words.next(), words.next()) {
                (None, None) => Some(Self::Tasks(None)),
                // A leading dash is an argument a task id never carries, so it
                // is answered with the usage line rather than looked up.
                (Some(id), None) if !id.starts_with('-') => Some(Self::Tasks(Some(id.to_string()))),
                _ => Some(Self::Usage("usage: /tasks [id|graph]")),
            },
            // The roster is the node's, not one session's, and the peer a
            // reader wants is chosen from the rows rather than named here.
            "/federation" => match words.next() {
                None => Some(Self::Federation),
                _ => Some(Self::Usage("usage: /federation")),
            },
            // The page belongs to the Host's browser, not to this line, so the
            // command names nothing: opening it shows what the tools read.
            "/browser" => match words.next() {
                None => Some(Self::Browser),
                _ => Some(Self::Usage("usage: /browser")),
            },
            // The argument is a row number or `all`, and the number is judged
            // where the roster is at hand rather than here: a run that finished
            // while the reader typed still answers with the count.
            "/subagents" => match words.next() {
                None => Some(Self::Subagents(None)),
                Some(argument) if !argument.starts_with('-') => {
                    Some(Self::Subagents(Some(argument.to_string())))
                }
                _ => Some(Self::Usage("usage: /subagents [number|all]")),
            },
            // The log is the open session's, so it has no argument either: the
            // session is chosen by opening it, not by naming it here.
            "/trajectory" => match words.next() {
                None => Some(Self::Trajectory),
                _ => Some(Self::Usage("usage: /trajectory")),
            },
            // A filter is free text, like `/audit`'s: it searches the ids and
            // the display names the extension host reports.
            "/extensions" => match rest {
                "" => Some(Self::Extensions(None)),
                filter => Some(Self::Extensions(Some(filter.to_string()))),
            },
            // A change is addressed by id or by path, both free text: a path
            // may carry spaces, and the Host resolves either against the same
            // changeset. An empty argument lists without building a diff.
            "/review" => match rest {
                "" => Some(Self::Review(None)),
                change => Some(Self::Review(Some(change.to_string()))),
            },
            // A number is the whole argument, but the parse does not judge it:
            // a row number is checked where the list it numbers is at hand, so
            // a stale or impossible number is answered with the count.
            "/trash" => match rest {
                "" => Some(Self::Trash(None)),
                number => Some(Self::Trash(Some(number.to_string()))),
            },
            "/restore" => match rest {
                "" => Some(Self::Restore(None)),
                number => Some(Self::Restore(Some(number.to_string()))),
            },
            "/purge" => match rest {
                "" => Some(Self::Purge(None)),
                number => Some(Self::Purge(Some(number.to_string()))),
            },
            // The `/goal` grammar is [`crate::state::parse_slash_line`]'s, not a
            // terminal invention: the same control words, matched case-
            // insensitively and only when they fill the whole argument, the
            // same `edit <objective>` prefix and the same verbatim objective.
            "/goal" => Some(goal_command(rest)),
            "/editor" => match words.next() {
                None => Some(Self::Editor),
                _ => Some(Self::Usage("usage: /editor · ^X")),
            },
            "/rename" => match rest {
                "" => Some(Self::Rename(None)),
                title => Some(Self::Rename(Some(title.to_string()))),
            },
            // The path is free text, like a title: the rest of the line is the
            // destination, and an empty one means "under the data directory".
            "/export" => match rest {
                "" => Some(Self::Export(None)),
                path => Some(Self::Export(Some(path.to_string()))),
            },
            "/queue" => match rest {
                "" => Some(Self::Queue(None)),
                "clear" => Some(Self::Queue(Some("clear".to_string()))),
                _ => Some(Self::Usage("usage: /queue · /queue clear")),
            },
            // A title is free text, like `/rename`'s: the rest of the line
            // names the new session, and an empty one names it after the clock.
            "/new" => match rest {
                "" => Some(Self::New(None)),
                title => Some(Self::New(Some(title.to_string()))),
            },
            // The argument is a number or a filter, both free text: a title
            // may carry spaces, so the rest of the line is taken verbatim.
            "/sessions" => match rest {
                "" => Some(Self::Sessions(None)),
                argument => Some(Self::Sessions(Some(argument.to_string()))),
            },
            // A search filter is free text, not a name: the palette's skill
            // field searches names and descriptions, and one exact name is
            // the skill a reader asked to read.
            "/skills" => match rest {
                "" => Some(Self::Skills(None)),
                filter => Some(Self::Skills(Some(filter.to_string()))),
            },
            // A note is free text, like a title: the rest of the line is the
            // note, and `clear` is the one control word that drops it.
            "/feedback" => match rest {
                "" => Some(Self::Feedback(None)),
                "clear" => Some(Self::Feedback(Some("clear".to_string()))),
                note => Some(Self::Feedback(Some(note.to_string()))),
            },
            // The argument is a call number or the one control word, both read
            // where the projection is at hand rather than here: a line that
            // names a call this session never made has to be answered with
            // the count, not by the parser.
            "/detail" => match rest {
                "" => Some(Self::Detail(None)),
                argument => Some(Self::Detail(Some(argument.to_string()))),
            },
            // The pane is the client's own shell, so it takes no argument: the
            // program is whatever the reader's login shell is.
            "/terminal" => match words.next() {
                None => Some(Self::Terminal),
                _ => Some(Self::Usage("usage: /terminal")),
            },
            // A section is a name, and an empty argument lists the whole rail:
            // the pane redraws itself from the Host either way, because the
            // settings a turn uses move when the desktop edits them.
            // The settings pane reads the whole rail when it is asked with no
            // argument; `/questions` takes none either way.
            "/questions" => match words.next() {
                None => Some(Self::Questions),
                Some(_) => Some(Self::Usage("usage: /questions")),
            },
            "/settings" => match words.next() {
                None => Some(Self::Settings(None)),
                Some(name) if rest.split_whitespace().count() == 1 => {
                    Some(Self::Settings(Some(name.to_string())))
                }
                Some(_) => Some(Self::Usage("usage: /settings [section]")),
            },
            // The key is one word - it is the row id the Host reports - while
            // the value is the rest of the line, because a verify command or a
            // base URL carries spaces and quotes that no argument splitter
            // should decide about.
            "/set" => match rest {
                "" => Some(Self::Set {
                    key: None,
                    value: None,
                }),
                text => {
                    let mut parts = text.splitn(2, char::is_whitespace);
                    let key = parts.next().unwrap_or_default().trim();
                    let value = parts
                        .next()
                        .map(str::trim)
                        .filter(|value| !value.is_empty());
                    Some(Self::Set {
                        key: Some(key.to_string()),
                        value: value.map(str::to_string),
                    })
                }
            },
            other => Some(Self::Unknown(other.to_string())),
        }
    }
}

/// The `/goal` grammar, mirrored from [`crate::state::parse_goal_request`].
///
/// The control words count only when they fill the whole argument, so
/// `/goal pause after verification` creates that literal objective; `edit` is
/// a prefix that takes the rest of the line, and anything else is the
/// objective verbatim. Case follows the desktop's: the words are matched
/// without case, the objective keeps the reader's own.
fn goal_command(rest: &str) -> SlashCommand {
    if rest.is_empty() {
        return SlashCommand::GoalShow;
    }
    match rest.to_lowercase().as_str() {
        "clear" => return SlashCommand::GoalClear,
        "pause" => return SlashCommand::GoalPause,
        "resume" => return SlashCommand::GoalResume,
        "edit" => return SlashCommand::Usage("usage: /goal edit <objective>"),
        _ => {}
    }
    if let Some(end) = rest.find(char::is_whitespace)
        && rest[..end].eq_ignore_ascii_case("edit")
    {
        let objective = rest[end..].trim();
        return if objective.is_empty() {
            SlashCommand::Usage("usage: /goal edit <objective>")
        } else {
            SlashCommand::GoalEdit(objective.to_string())
        };
    }
    SlashCommand::GoalSet(rest.to_string())
}

/// The lines this client submitted, kept the way a shell keeps its history.
///
/// It is client-side on purpose: the Host journals the turns it accepted as
/// durable state, while a recalled line is only text waiting in the composer.
#[derive(Debug, Default)]
struct PromptHistory {
    /// Submitted lines, oldest first.
    lines: Vec<String>,
    /// Position while browsing; `None` while composing a fresh line.
    at: Option<usize>,
    /// What the user was typing when browsing started.
    draft: String,
}

impl PromptHistory {
    /// Remember one submitted line. A repeat of the newest entry is that
    /// entry again, not a second one.
    fn record(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        if self.lines.last().map(String::as_str) != Some(line) {
            self.lines.push(line.to_string());
        }
        self.at = None;
        self.draft.clear();
    }

    /// The line one step further back, or `None` when already at the oldest.
    ///
    /// The first step saves what was being typed so the last step forward can
    /// restore it.
    fn older(&mut self, current: &str) -> Option<String> {
        let last = self.lines.len().checked_sub(1)?;
        let index = match self.at {
            None => {
                self.draft = current.to_string();
                last
            }
            Some(0) => return None,
            Some(index) => index - 1,
        };
        self.at = Some(index);
        self.lines.get(index).cloned()
    }

    /// The line one step forward, or the draft once past the newest entry.
    fn newer(&mut self) -> Option<String> {
        let at = self.at?;
        if at + 1 < self.lines.len() {
            self.at = Some(at + 1);
            return self.lines.get(at + 1).cloned();
        }
        self.at = None;
        Some(std::mem::take(&mut self.draft))
    }
}

/// The lines typed while a turn was running, waiting their turn.
///
/// The desktop holds them in its queue dock and drains one per finished turn;
/// the terminal keeps the same inbox client-side. It belongs to one session at
/// a time: the terminal shows one session's turns, so a line queued for another
/// session could never be sent from here, and [`App::flush_queue`] proves a
/// session idle only while it is the open one. A queue that belongs to another
/// session refuses a push rather than silently moving the inbox.
#[derive(Debug, Default)]
struct DraftQueue {
    /// The session these lines are waiting for; `None` while the queue is empty.
    session: Option<SessionId>,
    /// The lines, oldest first.
    lines: Vec<String>,
}

impl DraftQueue {
    /// Queue one line for `session`, or `false` when the queue already belongs
    /// to a different one and the line would be unsendable.
    fn push(&mut self, session: SessionId, text: &str) -> bool {
        match self.session {
            Some(queued) if queued != session => return false,
            Some(_) => {}
            None => self.session = Some(session),
        }
        self.lines.push(text.to_string());
        true
    }

    /// The session the queued lines are waiting for.
    fn session(&self) -> Option<SessionId> {
        self.session
    }

    /// The queued lines, oldest first.
    fn lines(&self) -> &[String] {
        &self.lines
    }

    /// How many lines wait, the count the dock's header reads.
    fn len(&self) -> usize {
        self.lines.len()
    }

    fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Drop every queued line, session and all: the dock and the next push start
    /// over.
    fn clear(&mut self) {
        self.lines.clear();
        self.session = None;
    }

    /// Take the oldest line. An emptied queue forgets its session, so the next
    /// push may belong to whichever session the reader opens.
    fn pop(&mut self) -> Option<String> {
        if self.lines.is_empty() {
            return None;
        }
        let line = self.lines.remove(0);
        if self.lines.is_empty() {
            self.session = None;
        }
        Some(line)
    }

    /// Put a line back at the head after a send the Host refused, so a failed
    /// dispatch never loses the reader's text.
    fn requeue(&mut self, text: String) {
        self.lines.insert(0, text);
    }
}

/// Whether an execution still holds its session, the one predicate both
/// [`App::busy`] and the live-text reconciliation read.
///
/// A terminal state means the Host will not run that execution again; only the
/// three live states can still emit activity or take an approval.
fn unfinished(status: ExecutionStatus) -> bool {
    matches!(
        status,
        ExecutionStatus::Queued | ExecutionStatus::Running | ExecutionStatus::WaitingApproval
    )
}

/// The byte offset where the line holding `cursor` starts.
///
/// The composer holds several lines - Shift-Enter, Ctrl-J and a pasted block
/// all insert one - so Home and Ctrl-A are line gestures rather than draft
/// ones: a reader fixing the second line of a prompt does not want the cursor
/// back at the top of it.
fn line_start(input: &str, cursor: usize) -> usize {
    let cursor = cursor.min(input.len());
    input[..cursor].rfind('\n').map_or(0, |index| index + 1)
}

/// The byte offset where the line holding `cursor` ends: its newline, or the
/// end of the draft.
fn line_end(input: &str, cursor: usize) -> usize {
    let cursor = cursor.min(input.len());
    input[cursor..]
        .find('\n')
        .map_or(input.len(), |index| cursor + index)
}

/// The byte offset of the word that starts before `cursor`: the whitespace the
/// cursor sits in is skipped first, then the run of non-whitespace before it.
fn word_start(input: &str, cursor: usize) -> usize {
    let head = input[..cursor.min(input.len())].trim_end();
    head.char_indices()
        .rev()
        .find(|(_, character)| character.is_whitespace())
        .map_or(0, |(index, character)| index + character.len_utf8())
}

/// The byte offset just past the next word: the whitespace the cursor sits in
/// is skipped, then the run of non-whitespace.
///
/// Landing after the word rather than on the space that follows it is what
/// makes a repeated step move instead of stalling in the gap.
fn word_end(input: &str, cursor: usize) -> usize {
    let cursor = cursor.min(input.len());
    let mut end = 0;
    let mut inside = false;
    for (index, character) in input[cursor..].char_indices() {
        if character.is_whitespace() && !inside {
            end = index + character.len_utf8();
            continue;
        }
        if character.is_whitespace() {
            break;
        }
        inside = true;
        end = index + character.len_utf8();
    }
    cursor + end
}

/// Delete the character under the cursor: the Delete key, the counterpart of
/// Backspace, which deletes the one before it.
fn delete_forward(input: &mut String, cursor: &mut usize) {
    let at = (*cursor).min(input.len());
    if let Some(character) = input[at..].chars().next() {
        input.replace_range(at..at + character.len_utf8(), "");
    }
    *cursor = at;
}

/// Kill from the cursor to the end of its line (Ctrl-K).
///
/// At a line's end there is nothing to the right but the newline, and joining
/// the two lines is what readline does there: the chord is never a no-op.
fn kill_to_line_end(input: &mut String, cursor: &mut usize) {
    let at = (*cursor).min(input.len());
    let end = line_end(input, at);
    let cut = if end > at {
        end
    } else if end < input.len() {
        end + 1
    } else {
        end
    };
    input.replace_range(at..cut, "");
    *cursor = at;
}

/// Kill from the start of the cursor's line to the cursor (Ctrl-U); at a line
/// start it takes the newline before it, which is Ctrl-K's rule backwards.
fn kill_to_line_start(input: &mut String, cursor: &mut usize) {
    let at = (*cursor).min(input.len());
    let start = line_start(input, at);
    let cut = if start < at {
        start
    } else {
        start.saturating_sub(1)
    };
    input.replace_range(cut..at, "");
    *cursor = cut;
}

/// Delete the word before the cursor (Ctrl-W), the chord a shell uses.
fn kill_word(input: &mut String, cursor: &mut usize) {
    let at = (*cursor).min(input.len());
    let start = word_start(input, at);
    input.replace_range(start..at, "");
    *cursor = start;
}

/// The composer block: the prompt wrapped to `width`, plus the row and column
/// inside that block the terminal cursor belongs on.
///
/// A prompt carries explicit line breaks (Shift-Enter inserts one) and a line
/// longer than the window wraps, so the block grows the way the desktop
/// composer does instead of hiding its tail off the right edge. Past
/// [`MAX_COMPOSER_ROWS`] the block scrolls within itself and the window always
/// keeps the cursor's row in view.
fn composer(input: &str, cursor: usize, width: usize) -> (Vec<String>, (usize, usize)) {
    let room = width.saturating_sub(2).max(1);
    // The cursor is a byte offset; the walk below counts characters.
    let at = input[..cursor.min(input.len())].chars().count();
    let mut rows: Vec<String> = vec![String::new()];
    let mut row = 0usize;
    let mut column = 0usize;
    let mut cursor_at = None;
    for (index, character) in input.chars().enumerate() {
        if index == at {
            cursor_at = Some((row, column));
        }
        if character == '\n' {
            rows.push(String::new());
            row += 1;
            column = 0;
            continue;
        }
        // Columns, not characters: a wide glyph takes two cells, and the
        // terminal's cursor is addressed in cells.
        let advance = UnicodeWidthChar::width(character).unwrap_or(0);
        if column + advance > room {
            rows.push(String::new());
            row += 1;
            column = 0;
        }
        rows[row].push(character);
        column += advance;
    }
    let cursor_at = cursor_at.unwrap_or((row, column));
    let (cursor_row, cursor_column) = cursor_at;
    let visible = rows.len().min(MAX_COMPOSER_ROWS);
    // The window moves only when the cursor would leave it, and never past the
    // end of the block.
    let top = cursor_row
        .saturating_sub(visible.saturating_sub(1))
        .min(rows.len().saturating_sub(visible));
    let lines = rows[top..top + visible]
        .iter()
        .enumerate()
        .map(|(offset, text)| {
            let prefix = if top + offset == 0 { "› " } else { "  " };
            truncate(&format!("{prefix}{text}"), width)
        })
        .collect();
    (lines, (cursor_row - top, 2 + cursor_column))
}

/// The plan block as the frame draws it, given the plan the Host projected.
///
/// The header carries the progress count and the step being worked on, because
/// a count alone cannot say what the model is doing now; the rows below it are
/// the steps themselves, capped so the block never grows past the prompt's
/// share of the frame. An empty plan draws nothing at all.
fn plan_block(plan: &[PlanItemView], width: usize) -> Vec<String> {
    if plan.is_empty() {
        return Vec::new();
    }
    let total = plan.len();
    let done = plan
        .iter()
        .filter(|item| item.status == PlanItemStatus::Completed)
        .count();
    let current = plan
        .iter()
        .find(|item| item.status == PlanItemStatus::InProgress)
        .map(|item| item.text.as_str());
    let header = match current {
        Some(text) => format!("plan {done}/{total} · {text}"),
        None => format!("plan {done}/{total}"),
    };
    let mut lines = vec![truncate(&header, width)];
    let shown = total.min(MAX_PLAN_ROWS);
    lines.extend(plan.iter().take(shown).map(|item| {
        truncate(
            &format!("  {} {}", plan_glyph(item.status), item.text),
            width,
        )
    }));
    if total > shown {
        lines.push(truncate(&format!("  … {} more", total - shown), width));
    }
    lines
}

/// One plan step's glyph, the terminal's form of the desktop panel's marks.
///
/// The three states are the model's own vocabulary: a step is untouched, being
/// worked on, or finished, and a reader scanning the block reads the glyph
/// before the words.
fn plan_glyph(status: PlanItemStatus) -> char {
    match status {
        PlanItemStatus::Pending => '○',
        PlanItemStatus::InProgress => '●',
        PlanItemStatus::Completed => '✓',
    }
}

/// Whether a `/feedback` argument drops the note rather than writing one.
///
/// Case does not matter, the way every other control word in the slash
/// grammar is matched; a note that happens to begin with the word is a note,
/// because the control word fills the whole argument.
fn clears_feedback(argument: &str) -> bool {
    argument.to_lowercase() == "clear"
}

/// The model, preset, permission and plan the session runs with, as one line.
/// One row of `/sessions`: the number that opens it, its title, the newest
/// message, and the marks the desktop's session menu draws.
///
/// The number is the row's index in the whole list, not in a filtered one: a
/// filter narrows what is shown, and a row keeps the number that opens it.
/// The row a completion row's id names in the snapshot the picker ranked.
///
/// `None` means the draft named a chat this client no longer lists - a session
/// another client parked, or a list that has moved since the picker opened.
fn session_index(sessions: &[SessionSummary], id: &str) -> Option<usize> {
    let id = id.parse::<SessionId>().ok()?;
    sessions.iter().position(|session| session.session_id == id)
}

/// The folder this client's workspace reads follow: the Host's answer when the
/// Host named one, the launching directory otherwise.
///
/// One Host serves one workspace, so when the terminal attached to a Host
/// another process started, the folder this terminal was launched in is not
/// necessarily the folder turns run in. The Host's catalogue names the root
/// every turn resolves, and that answer wins; only a Host that names no root
/// (an executor with no workspace surface) falls back to the directory this
/// client opened with, so `@`, `/files` and the shell never point at a tree no
/// turn can reach.
fn host_workspace(host_root: &str, launched: Option<PathBuf>) -> Option<PathBuf> {
    let root = host_root.trim();
    if root.is_empty() {
        return launched;
    }
    Some(PathBuf::from(root))
}

/// The workspace's display name, the detail the desktop's `@` card draws on a
/// session row.
///
/// The terminal client is attached to one Host, and the Host names one
/// workspace directory, so the directory's own name is the honest label; a
/// client that names no directory says so rather than inventing a title.
fn workspace_label(root: Option<&Path>) -> String {
    root.and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "no workspace".to_string())
}

fn session_row(
    index: usize,
    session: &SessionSummary,
    current: bool,
    now: DateTime<Utc>,
) -> String {
    let marker = if current { "›" } else { " " };
    // The desktop sidebar marks a pinned chat with a star; a terminal spells
    // the same state as a word beside the age, because a glyph that means
    // nothing to a monochrome screen would be the only place it is said.
    let pinned = if session.pinned { " · pinned" } else { "" };
    let state = match session.active_execution {
        Some(ExecutionStatus::WaitingApproval) => " · waiting approval",
        Some(_) => " · running",
        None => "",
    };
    let approvals = match session.pending_approvals {
        0 => String::new(),
        1 => " · 1 approval".to_string(),
        count => format!(" · {count} approvals"),
    };
    let preview = session
        .preview
        .as_deref()
        .map(str::trim)
        .filter(|preview| !preview.is_empty())
        .map(|preview| format!(" — {preview}"))
        .unwrap_or_default();
    format!(
        "{marker}{}. {title}{preview} · {} · {} msg{state}{approvals}{pinned}",
        index + 1,
        age(session.updated_at, now),
        session.message_count,
        title = session.title
    )
}

/// How many rows one `/find` list offers.
///
/// A common word sits on more rows than a list a reader has to walk is worth,
/// so the list is a window: the status row says how many matched, and the row a
/// reader wants is usually in the newest part of the conversation.
const MAX_FIND_ROWS: usize = 20;

/// The rows of `document` that contain `text`, numbered from one, with the text
/// a list row shows.
///
/// A match ignores case, because a reader remembering a word rarely remembers
/// how it was capitalised, and the number counts from one the way every other
/// listing in this client numbers its rows.
fn find_rows(document: &[(String, Option<usize>)], text: &str) -> Vec<(usize, String)> {
    let needle = text.to_lowercase();
    document
        .iter()
        .enumerate()
        .filter(|(_, (row, _))| row.to_lowercase().contains(&needle))
        .map(|(index, (row, _))| (index + 1, row.trim().to_string()))
        .collect()
}

/// The rows a `/find` list paints: one per hit, and the numbered row as the
/// fact beside the line that carries the text.
fn find_items(hits: &[(usize, String)]) -> Vec<picker::Item> {
    hits.iter()
        .take(MAX_FIND_ROWS)
        .map(|(number, text)| picker::Item {
            label: text.clone(),
            detail: format!("row {number}"),
            choice: picker::Choice::Find(number - 1),
            active: false,
        })
        .collect()
}

/// The scroll that leaves document row `row` in the middle of a body `rows`
/// tall, for a document that is bottom-aligned.
///
/// The window is the last `rows` rows of the document, `scroll` above its end,
/// so landing on a row means choosing where that end sits: half a screen past
/// the hit puts it near the middle, which is where a reader wants a line they
/// jumped to rather than against the edge of the screen.
fn reveal_scroll(total: usize, row: usize, rows: usize) -> usize {
    let end = (row + 1 + rows / 2).min(total);
    total.saturating_sub(end)
}

/// The rows `/sessions` offers as a list: one per session of the snapshot, in
/// the order the printed listing numbers them.
///
/// A row's `active` tick marks the chat that is already open, which is the
/// question the desktop's session menu answers with its highlight: the list
/// then says both where the reader is and where they might go.
fn session_items(
    sessions: &[SessionSummary],
    tick: Option<usize>,
    kind: picker::Kind,
) -> Vec<picker::Item> {
    let now = Utc::now();
    sessions
        .iter()
        .enumerate()
        .map(|(index, session)| picker::Item {
            label: session.title.clone(),
            detail: session_detail(session, now),
            // The list the row came from decides which command its number is:
            // the same index opens a session, takes one out of the bin, or
            // brings one back from the archive.
            choice: match kind {
                picker::Kind::Trash => picker::Choice::Trash(index),
                picker::Kind::Archived => picker::Choice::Archived(index),
                _ => picker::Choice::Session(index),
            },
            active: tick == Some(index),
        })
        .collect()
}

/// The facts beside a session's title: how recent it is, how much it holds,
/// and what it is doing.
///
/// The same four the printed row carries in words, kept to the right edge of a
/// list row so they read as one column down the screen.
fn session_detail(session: &SessionSummary, now: DateTime<Utc>) -> String {
    let mut detail = format!(
        "{} · {} msg",
        age(session.updated_at, now),
        session.message_count
    );
    if session.pinned {
        detail.push_str(" · pinned");
    }
    match session.active_execution {
        Some(ExecutionStatus::WaitingApproval) => detail.push_str(" · waiting approval"),
        Some(_) => detail.push_str(" · running"),
        None => {}
    }
    match session.pending_approvals {
        0 => {}
        1 => detail.push_str(" · 1 approval"),
        count => detail.push_str(&format!(" · {count} approvals")),
    }
    detail
}

/// A relative age, the way the desktop's session menu shows one.
///
/// Seconds only up to a minute, then minutes, hours and days: a list row has
/// to say which session is recent, not hand the reader a timestamp to
/// subtract.
fn age(then: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let seconds = (now - then).num_seconds().max(0);
    match seconds {
        0..=59 => "now".to_string(),
        60..=3599 => format!("{}m", seconds / 60),
        3600..=86399 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86400),
    }
}

/// The `/trash` status line: what is parked, or how to park something.
///
/// The bin is a list the reader acts on by number, so the line carries the
/// rows and the two gestures that address them. An empty bin says where a chat
/// comes from instead of printing nothing: parking starts at `/sessions`.
fn trash_summary(rows: &[String], total: usize) -> String {
    if rows.is_empty() {
        return "the bin is empty · /trash <number> parks the row /sessions numbers".to_string();
    }
    let shown = rows
        .iter()
        .take(10)
        .cloned()
        .collect::<Vec<_>>()
        .join(" | ");
    let more = rows.len().saturating_sub(10);
    let tail = if more > 0 {
        format!(" | … {more} more")
    } else {
        String::new()
    };
    format!("bin: {total} parked · {shown}{tail} | /restore <n> restores · /purge <n> drops")
}

/// Whether a session answers a `/sessions` filter: its title or its newest
/// message, matched without case, the two fields the desktop's menu searches.
fn session_matches(session: &SessionSummary, query: &str) -> bool {
    let query = query.to_lowercase();
    session.title.to_lowercase().contains(&query)
        || session
            .preview
            .as_deref()
            .is_some_and(|preview| preview.to_lowercase().contains(&query))
}

/// The `/sessions archived` status line, the Archived filter's rows.
///
/// A separate line from `/sessions`: the two lists answer different questions,
/// and the archived one says which verb takes a row back out.
fn archived_summary(rows: &[String], total: usize, query: Option<&str>) -> String {
    match (rows.is_empty(), query) {
        (true, Some(query)) => format!("no archived session matches '{query}' · {total} archived"),
        (true, None) => "no archived sessions · /sessions archive <number> hides one".to_string(),
        (false, _) => {
            let shown = rows
                .iter()
                .take(10)
                .cloned()
                .collect::<Vec<_>>()
                .join(" | ");
            let more = rows.len().saturating_sub(10);
            let tail = if more > 0 {
                format!(" | … {more} more")
            } else {
                String::new()
            };
            format!(
                "archived: {} of {total} · {shown}{tail} | /sessions unarchive <n> brings one back",
                rows.len()
            )
        }
    }
}

/// The `/sessions` status line: how many rows matched, then the rows.
///
/// The status row is one line, so at most ten rows ride on it and the rest
/// fold into a count. Filtering never renumbers the rows, so a number a reader
/// read here still opens the session it named after the next listing.
fn session_summary(rows: &[String], total: usize, query: Option<&str>) -> String {
    match (rows.is_empty(), query) {
        (true, Some(query)) => {
            format!("no session matches '{query}' · {total} in this workspace")
        }
        (true, None) => "no sessions in this workspace · Ctrl-T creates one".to_string(),
        (false, _) => {
            let shown = rows
                .iter()
                .take(10)
                .cloned()
                .collect::<Vec<_>>()
                .join(" | ");
            let mut line = format!("{} of {total} session(s): {shown}", rows.len());
            let more = rows.len().saturating_sub(10);
            if more > 0 {
                line.push_str(&format!(" | … {more} more"));
            }
            line
        }
    }
}

/// One queued line's preview, the terminal's form of the desktop's `previewOf`.
///
/// Blocks join with a space, runs of whitespace collapse to one, and the first
/// [`QUEUE_PREVIEW_CHARS`] characters survive; anything longer gets an ellipsis.
/// The budget counts characters rather than bytes, so a preview never splits a
/// multi-byte character.
fn queue_preview(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = flat.chars().collect();
    if chars.len() > QUEUE_PREVIEW_CHARS {
        format!(
            "{}…",
            chars[..QUEUE_PREVIEW_CHARS].iter().collect::<String>()
        )
    } else {
        flat
    }
}

/// The queue dock as the frame draws it, or nothing while no line waits.
///
/// The desktop draws a count header only when more than one message waits, and
/// the terminal does the same: a single waiting line reads as itself. Rows are
/// numbered so `/queue` and `/queue clear` can name them, and past
/// [`MAX_QUEUE_ROWS`] the rest fold into one counted row so the dock can never
/// take the transcript's rows.
fn queue_block(queue: &DraftQueue, width: usize) -> Vec<String> {
    if queue.is_empty() {
        return Vec::new();
    }
    let total = queue.len();
    let mut lines = Vec::with_capacity(MAX_QUEUE_ROWS + 2);
    if total > 1 {
        lines.push(truncate(
            &format!("queued {total} · sends when the turn ends"),
            width,
        ));
    }
    let shown = total.min(MAX_QUEUE_ROWS);
    lines.extend(
        queue
            .lines()
            .iter()
            .take(shown)
            .enumerate()
            .map(|(index, line)| {
                truncate(&format!("  {}. {}", index + 1, queue_preview(line)), width)
            }),
    );
    if total > shown {
        lines.push(truncate(&format!("  … {} more", total - shown), width));
    }
    lines
}

/// The title a new session carries: the reader's own words, trimmed, or the
/// clock when nothing was typed.
///
/// The Host refuses an empty title, so whitespace is no title here rather than
/// a command the Host would reject; a title's own inner spaces stay where the
/// reader put them, the same as `/rename`'s.
fn session_title(title: Option<&str>) -> String {
    title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("Session {}", chrono::Local::now().format("%H:%M")))
}

/// One skill row: the `/` gesture that invokes it, marked when its manifest
/// restricts the skill to the reader.
fn skill_row(skill: &SkillView) -> String {
    if skill.user_only {
        format!("/{} (user-only)", skill.name)
    } else {
        format!("/{}", skill.name)
    }
}

/// One skill in full: the gesture, its one-line description and who may invoke
/// it.
///
/// The marker is the manifest's `disable-model-invocation` fact, which is the
/// one thing a reader has to know before typing the gesture into a turn: a
/// model-chosen skill runs without the reader naming it.
fn skill_detail(skill: &SkillView) -> String {
    let invocation = if skill.user_only {
        "user-invoked only"
    } else {
        "model may invoke"
    };
    match skill.description.trim() {
        "" => format!("/{} · {invocation}", skill.name),
        description => format!("/{} · {description} · {invocation}", skill.name),
    }
}

/// Whether a palette search matches one skill.
///
/// The desktop's skill field ranks by name and description; the terminal has
/// one status row instead of a list, so a case-insensitive substring of either
/// half is what it can honour - and it stays predictable enough to type.
fn skill_matches(skill: &SkillView, filter: &str) -> bool {
    let filter = filter.to_lowercase();
    skill.name.to_lowercase().contains(&filter)
        || skill.description.to_lowercase().contains(&filter)
}

/// One roster row: a server's name and its state.
fn mcp_row(server: &McpServerView) -> String {
    format!("{} {}", server.name, mcp_state(server))
}

/// One server in full: where it came from, how it is reached, its state and -
/// once connected - the exact tools the model can call.
///
/// The transport is the launch description the settings carry, minus
/// credentials; the source separates a settings entry from a server an installed
/// extension declares, which is the one distinction an operator has to act on.
fn mcp_detail(server: &McpServerView) -> String {
    let source = if server.extension {
        "extension"
    } else {
        "settings"
    };
    let mut line = format!(
        "mcp {} · {source} · {} · {}",
        server.name,
        server.transport,
        mcp_state(server)
    );
    if !server.tools.is_empty() {
        line.push_str(&format!(" · tools: {}", server.tools.join(", ")));
    }
    line
}

/// How one MCP server's state reads in the terminal's roster.
///
/// The same three facts the desktop's pane separates: switched off, connected
/// with the tools it offers, or not connected and the reason why. A server that
/// connected and advertises nothing says exactly that instead of reading as
/// ready, which is the difference a reader needs when a tool is missing.
fn mcp_state(server: &McpServerView) -> String {
    if let Some(error) = server.error.as_deref() {
        return format!("not connected: {error}");
    }
    if !server.enabled {
        return "disabled".to_string();
    }
    match server.tools.len() {
        0 => "connected, no tools".to_string(),
        count => format!("{count} tool(s)"),
    }
}

/// One record as the Memory pane lists it: the text, then the facts the
/// desktop card carries in its pills and footer.
///
/// A recall hit also carries its score and named contributions, drawn between
/// the text and the footer where the desktop's hit card puts them; a record
/// whose lifecycle is not `active` names that state, because a record that no
/// longer steers a turn is exactly what a reader browsing history asks about.
fn memory_row(view: &MemoryView, width: usize, now: DateTime<Utc>) -> Vec<String> {
    let content: String = view.content.chars().take(MEMORY_CONTENT_CHARS).collect();
    let mut rows = wrap("• ", content.trim(), width);
    if let Some(score) = view.score {
        let mut line = format!("  score {score:.2}");
        let reasons = view.reasons.trim();
        if !reasons.is_empty() {
            line.push_str(&format!(" · {reasons}"));
        }
        rows.push(truncate(&line, width));
    }
    let mut meta = format!(
        "  {} · {}:{} · {} · rev {} · {}",
        view.kind,
        view.scope,
        view.scope_id,
        view.source,
        view.revision,
        age(view.updated_at, now)
    );
    if view.lifecycle != "active" {
        meta.push_str(&format!(" · {}", view.lifecycle));
    }
    rows.push(truncate(&meta, width));
    rows
}

/// The Memory pane's body: the store's lifecycle, every row, and the one line
/// that explains an empty report.
///
/// The state is stated rather than inferred from an empty list: memory that is
/// switched off and memory that holds nothing are different answers, and the
/// desktop pane distinguishes them the same way. A search that the store
/// skipped for a reason says so, because "no match" and "never searched" are
/// not the same answer either.
fn memory_body(
    report: &MemoryReport,
    query: Option<&str>,
    width: usize,
    now: DateTime<Utc>,
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    match report.state.as_str() {
        "disabled" => lines.extend(wrap(
            "! ",
            "memory is switched off in this workspace's settings; nothing is listed or recalled",
            width,
        )),
        "init_failed" => lines.extend(wrap(
            "! ",
            &format!(
                "the memory store could not be opened: {}",
                report.error.as_deref().unwrap_or("no detail")
            ),
            width,
        )),
        _ => {}
    }
    if report.memories.is_empty() {
        match report.state.as_str() {
            "disabled" | "init_failed" => {}
            _ => match query {
                Some(query) => {
                    lines.extend(wrap("  ", &format!("no record matched \"{query}\""), width))
                }
                None => lines.extend(wrap("  ", "the store holds no record yet", width)),
            },
        }
    }
    for view in &report.memories {
        lines.extend(memory_row(view, width, now));
    }
    // A search can be skipped before it runs - a store without a recall
    // backend, or one still indexing. The reason is the difference between an
    // empty answer and an answer that was never computed.
    if report.memories.is_empty()
        && let Some(skip) = report
            .error
            .as_deref()
            .map(str::trim)
            .filter(|skip| !skip.is_empty())
        && report.state != "init_failed"
    {
        lines.extend(wrap("  ", &format!("recall skipped · {skip}"), width));
    }
    lines
}

/// The Memory pane's title row: what it shows and how to leave.
fn memory_title(pane: &MemoryPane) -> String {
    let count = pane.report.memories.len();
    match pane.query.as_deref() {
        Some(query) => {
            format!("memory · recall \"{query}\" · {count} hit(s) · Up/Down scrolls · Esc closes")
        }
        None => format!("memory · {count} record(s) · Up/Down scrolls · Esc closes"),
    }
}

/// The Memory pane: its title, then the body windowed to `rows`.
///
/// The title stays on the first row while the body scrolls, so a reader who
/// scrolled into a long listing still knows what they are reading and how to
/// get back to the transcript.
fn memory_pane(pane: &MemoryPane, width: usize, rows: usize) -> Vec<String> {
    let rows = rows.max(1);
    let body_rows = rows.saturating_sub(1);
    let body = memory_body(&pane.report, pane.query.as_deref(), width, Utc::now());
    let total = body.len();
    let start = pane.scroll.min(total.saturating_sub(body_rows));
    let end = (start + body_rows).min(total);
    let mut lines = Vec::with_capacity(rows);
    lines.push(truncate(&memory_title(pane), width));
    lines.extend(body[start..end].iter().cloned());
    lines.resize(rows, String::new());
    lines
}

/// The `/memory` status line: what the pane is showing and how to leave it.
fn memory_summary(report: &MemoryReport, query: Option<&str>) -> String {
    let count = report.memories.len();
    let mut line = match (count, query) {
        (0, Some(query)) => format!("memory · no hit for \"{query}\""),
        (0, None) => "memory · the store is empty".to_string(),
        (count, Some(query)) => format!("memory · {count} hit(s) for \"{query}\""),
        (count, None) => format!("memory · {count} record(s)"),
    };
    match report.state.as_str() {
        "disabled" => line.push_str(" · memory disabled"),
        "init_failed" => line.push_str(" · store failed"),
        _ => {}
    }
    if let Some(reason) = report
        .error
        .as_deref()
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
    {
        line.push_str(&format!(" · {reason}"));
    }
    line.push_str(" · Esc closes · Up/Down scrolls");
    line
}

/// The Settings pane's body: the rail's sections, each under its own header.
///
/// The rows are the Host's own resolution, not a second read of the settings
/// files: a label, the effective value and a one-word provenance are all a
/// terminal row has space for, and the provenance is what lets "settings",
/// "workspace" and "vault" tell apart. A named section narrows the body; an
/// unknown name is answered with the names the Host does have, so a typo gets
/// the rail instead of an empty frame.
fn settings_body(report: &SettingsReport, section: Option<&str>, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    if let Some(reason) = report
        .error
        .as_deref()
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
    {
        lines.extend(wrap("! ", reason, width));
    }
    if report.state != "ready" && report.error.is_none() {
        lines.extend(wrap(
            "! ",
            &format!(
                "the Host resolves its settings as {}; nothing editable is listed",
                report.state
            ),
            width,
        ));
    }
    let sections: Vec<&SettingsSectionView> = match section {
        Some(name) => report
            .sections
            .iter()
            .filter(|section| section.section == name)
            .collect(),
        None => report.sections.iter().collect(),
    };
    if sections.is_empty() {
        // A refused name already travels in the error, which names the cells
        // the Host does have; only a report with no error needs this pane's
        // own wording.
        if let Some(name) = section {
            if report.error.is_none() {
                lines.extend(wrap(
                    "  ",
                    &format!(
                        "no section is named \"{name}\"; the Host lists {}",
                        settings_names(report)
                    ),
                    width,
                ));
            }
        } else if report.state == "ready" {
            lines.extend(wrap("  ", "the settings resolve to no section", width));
        }
        return lines;
    }
    // One column width for the whole body, so a reader scanning two sections
    // reads the values down one edge instead of re-finding the column per cell.
    let label_width = sections
        .iter()
        .flat_map(|section| section.rows.iter())
        .map(|row| cells(&row.label))
        .max()
        .unwrap_or(0)
        .clamp(2, 20);
    for section in &sections {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(truncate(
            &format!(
                "{} · {} row(s)",
                section.section.to_uppercase(),
                section.rows.len()
            ),
            width,
        ));
        if section.rows.is_empty() {
            lines.extend(wrap("  ", "this section has no row", width));
        }
        for row in &section.rows {
            lines.push(settings_row(row, label_width, width));
        }
    }
    lines
}

/// One settings row: the label, the effective value, and where it came from.
fn settings_row(row: &SettingsRowView, label_width: usize, width: usize) -> String {
    let value = row.value.trim();
    let value = if value.is_empty() { "—" } else { value };
    // A star is this pane's only affordance: the row a write can address is the
    // row that carries a key, so marking it here is what tells a reader which
    // of the dozens of values they may change.
    let marker = if row.key.is_some() { "* " } else { "  " };
    let mut line = format!(
        "{marker}{}{}  {value}",
        row.label,
        " ".repeat(label_width.saturating_sub(cells(&row.label)))
    );
    let source = row.source.trim();
    if !source.is_empty() {
        line.push_str(&format!(" ({source})"));
    }
    truncate(&line, width)
}

/// Every row a write can address, in rail order.
fn editable_rows(report: &SettingsReport) -> Vec<&SettingsRowView> {
    report
        .sections
        .iter()
        .flat_map(|section| section.rows.iter())
        .filter(|row| row.key.is_some())
        .collect()
}

/// What one write changed, as the row the Host reports now.
///
/// The summary is read back from the report rather than from the text that was
/// typed: a capacity of `8K` is a write of 8192, and the reader should see the
/// value the next turn will actually use.
fn setting_summary(report: &SettingsReport, key: &str) -> String {
    let row = report
        .sections
        .iter()
        .flat_map(|section| section.rows.iter())
        .find(|row| row.key.as_deref() == Some(key));
    match row {
        Some(row) => {
            let source = row.source.trim();
            if source.is_empty() {
                format!("{} = {} · the next turn uses it", row.label, row.value)
            } else {
                format!(
                    "{} = {} ({source}) · the next turn uses it",
                    row.label, row.value
                )
            }
        }
        None => format!("{key} was written; the Host lists no row for it"),
    }
}

/// The section names the Host lists, for the answer to an unknown name.
fn settings_names(report: &SettingsReport) -> String {
    if report.sections.is_empty() {
        return "no section".to_string();
    }
    report
        .sections
        .iter()
        .map(|section| section.section.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The Settings pane's title: what it lists and how to leave it.
fn settings_title(pane: &SettingsPane) -> String {
    let report = &pane.report;
    let rows: usize = report
        .sections
        .iter()
        .map(|section| section.rows.len())
        .sum();
    let editable = editable_rows(report).len();
    let mut title = match pane.section.as_deref() {
        Some(name) => match report
            .sections
            .iter()
            .find(|section| section.section == name)
        {
            Some(section) => format!(
                "settings · {name} · {} row(s) · {editable} writable",
                section.rows.len()
            ),
            // A name the rail does not hold is said on the title too, so the
            // refusal is visible while the body is scrolled away.
            None => format!("settings · {name} · not a section"),
        },
        None => format!(
            "settings · {} section(s) · {rows} row(s) · {editable} writable",
            report.sections.len()
        ),
    };
    if report.state != "ready" {
        title.push_str(&format!(" · {}", report.state));
    }
    title.push_str(" · /set <key> <value> writes · Up/Down scrolls · Esc closes");
    title
}

/// The Settings pane: its title, then the rail windowed to `rows`.
fn settings_pane(pane: &SettingsPane, width: usize, rows: usize) -> Vec<String> {
    let rows = rows.max(1);
    let body_rows = rows.saturating_sub(1);
    let body = settings_body(&pane.report, pane.section.as_deref(), width);
    let total = body.len();
    let start = pane.scroll.min(total.saturating_sub(body_rows));
    let end = (start + body_rows).min(total);
    let mut lines = Vec::with_capacity(rows);
    lines.push(truncate(&settings_title(pane), width));
    lines.extend(body[start..end].iter().cloned());
    lines.resize(rows, String::new());
    lines
}

/// The `/settings` status line: what the pane is showing and how to leave it.
fn settings_summary(report: &SettingsReport, section: Option<&str>) -> String {
    let rows: usize = report
        .sections
        .iter()
        .map(|section| section.rows.len())
        .sum();
    let mut line = match section {
        Some(name) => match report
            .sections
            .iter()
            .find(|section| section.section == name)
        {
            Some(section) => format!(
                "settings · {} · {} row(s)",
                section.section,
                section.rows.len()
            ),
            // The status line's first words are what the command reads to pick
            // the level, so a refused name states itself plainly. The Host's
            // own refusal already names the cells it has, so it is used when
            // present instead of a second roster that may have none.
            None => report
                .error
                .as_deref()
                .map(str::trim)
                .filter(|reason| !reason.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    format!(
                        "no settings section \"{name}\" · known: {}",
                        settings_names(report)
                    )
                }),
        },
        None => format!(
            "settings · {} section(s) · {rows} row(s)",
            report.sections.len()
        ),
    };
    if report.state != "ready" {
        line.push_str(&format!(" · {}", report.state));
    }
    if let Some(reason) = report
        .error
        .as_deref()
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
    {
        line.push_str(&format!(" · {reason}"));
    }
    line.push_str(" · Up/Down scrolls · Esc closes");
    line
}

/// Whether the report is a directory listing rather than one file or a refusal.
fn files_is_listing(report: &WorkspaceReport) -> bool {
    report.state == "dir"
}

/// The path a title states: the Host's own relative path, or `/` for the root it
/// was asked about.
fn files_place(report: &WorkspaceReport) -> &str {
    let path = report.path.trim();
    if path.is_empty() { "/" } else { path }
}

/// Join one listing row onto the directory the Host answered with.
///
/// The client sends what the Host gave it plus one name, and the Host resolves
/// that against its own root. An empty parent is the root, so its child is the
/// bare name.
fn join_path(parent: &str, name: &str) -> String {
    let parent = parent.trim();
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

/// The directory one level above `path`; the root is its own parent, so Enter on
/// a root document reopens the root instead of failing.
fn parent_path(path: &str) -> String {
    let path = path.trim();
    match path.rfind('/') {
        Some(index) => path[..index].to_string(),
        None => String::new(),
    }
}

/// The size a row shows, by the desktop file manager's own rule: a directory has
/// none, and a file's bytes are grouped in KB/MB/GB rather than counted raw.
fn file_size_label(size: u64, dir: bool) -> String {
    if dir {
        return String::new();
    }
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = size as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        return format!("{size} {}", UNITS[unit]);
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// One listing row: the cursor, the name - a directory keeps its trailing slash
/// so it reads as one without a second column - and the size held at the row's
/// right edge until the name would touch it.
fn files_row(entry: &WorkspaceEntryView, selected: bool, width: usize) -> String {
    let marker = if selected { "› " } else { "  " };
    let name = if entry.dir {
        format!("{}/", entry.name)
    } else {
        entry.name.clone()
    };
    let size = file_size_label(entry.size, entry.dir);
    let marker_width = cells(marker);
    if size.is_empty() {
        return truncate(&format!("{marker}{name}"), width);
    }
    let size_width = cells(&size);
    // One column of air between the name and the size, so a long name ends
    // before the figure instead of running into it.
    let name_room = width.saturating_sub(marker_width + size_width + 1).max(1);
    let name = truncate(&name, name_room);
    let used = marker_width + cells(&name) + size_width;
    let pad = width.saturating_sub(used).max(1);
    format!("{marker}{name}{}{size}", " ".repeat(pad))
}

/// The Files pane's rows: one directory's listing, one file's text, or the
/// reason the Host could not answer.
///
/// A listing row carries the cursor, because a terminal selects a row instead of
/// clicking one; a document carries its own lines and nothing else, which is
/// what a reader opened it to see, with tabs expanded so a source row lands in
/// the same columns it would in the desktop editor.
fn files_body(report: &WorkspaceReport, selection: usize, width: usize) -> Vec<String> {
    match report.state.as_str() {
        "dir" => {
            if report.entries.is_empty() {
                let place = files_place(report).to_string();
                return wrap("  ", &format!("{place} holds nothing"), width);
            }
            let selection = selection.min(report.entries.len().saturating_sub(1));
            report
                .entries
                .iter()
                .enumerate()
                .map(|(index, entry)| files_row(entry, index == selection, width))
                .collect()
        }
        "file" => {
            let content = report.content.as_deref().unwrap_or_default();
            let mut lines: Vec<String> = content
                .lines()
                .map(|line| truncate(&line.replace('\t', "    "), width))
                .collect();
            if lines.is_empty() {
                return wrap("  ", "this file is empty", width);
            }
            if report.truncated {
                lines.push(String::new());
                lines.extend(wrap(
                    "  ",
                    "the file continues past what this pane shows",
                    width,
                ));
            }
            lines
        }
        _ => {
            let why = report
                .error
                .as_deref()
                .map(str::trim)
                .filter(|why| !why.is_empty())
                .unwrap_or("the Host could not read that path");
            wrap("  ", why, width)
        }
    }
}

/// The Files pane's title row: what it shows, its own numbers, and how to leave.
fn files_title(pane: &FilesPane) -> String {
    let report = &pane.report;
    let place = files_place(report);
    match report.state.as_str() {
        "dir" => format!(
            "files · {place} · {} row(s) · Up/Down moves · Enter opens · Esc closes",
            report.entries.len()
        ),
        "file" => {
            let lines = report
                .content
                .as_deref()
                .map_or(0, |content| content.lines().count());
            let head = if report.truncated {
                " · head only"
            } else {
                ""
            };
            format!(
                "files · {place} · {lines} line(s){head} · Up/Down scrolls · Enter up · Esc closes"
            )
        }
        state => format!("files · {place} · {state} · Esc closes"),
    }
}

/// The Files pane: its title, then the body windowed to `rows`.
///
/// A listing windows on the cursor, the way a file manager does: walking down
/// keeps the highlighted row in the last visible slot instead of scrolling past
/// it. A document windows on its own offset, so Enter-up and `/files` return to
/// the top of the directory.
fn files_pane(pane: &FilesPane, width: usize, rows: usize) -> Vec<String> {
    let rows = rows.max(1);
    let body_rows = rows.saturating_sub(1);
    let body = files_body(&pane.report, pane.selection, width);
    let total = body.len();
    let start = if files_is_listing(&pane.report) {
        pane.selection
            .saturating_sub(body_rows.saturating_sub(1))
            .min(total.saturating_sub(body_rows))
    } else {
        pane.scroll.min(total.saturating_sub(body_rows))
    };
    let end = (start + body_rows).min(total);
    let mut lines = Vec::with_capacity(rows);
    lines.push(truncate(&files_title(pane), width));
    lines.extend(body[start..end].iter().cloned());
    lines.resize(rows, String::new());
    lines
}

/// The `/files` status line: what was opened and how to leave it.
fn files_summary(report: &WorkspaceReport) -> String {
    let place = files_place(report);
    let mut line = match report.state.as_str() {
        "dir" => format!("files · {place} · {} row(s)", report.entries.len()),
        "file" => {
            let lines = report
                .content
                .as_deref()
                .map_or(0, |content| content.lines().count());
            let head = if report.truncated {
                " · head only"
            } else {
                ""
            };
            format!("files · {place} · {lines} line(s){head}")
        }
        state => format!("files · {place} · {state}"),
    };
    if let Some(why) = report
        .error
        .as_deref()
        .map(str::trim)
        .filter(|why| !why.is_empty())
    {
        line.push_str(&format!(" · {why}"));
    }
    line.push_str(" · Esc closes");
    line
}

/// Whether a diff view carries one path's comparison rather than the list.
fn diff_is_patch(view: &WorkspaceDiffView) -> bool {
    view.state == "patch"
}

/// Whether the view is the Host saying it could not compare anything.
///
/// The four refusals are kept apart from `clean` and `listed` because they say
/// nothing about the working copy: a client that drew "nothing changed" for a
/// root that is not a checkout would be answering a question nobody asked.
fn diff_refusal(view: &WorkspaceDiffView) -> bool {
    matches!(
        view.state.as_str(),
        "unknown" | "refused" | "unreadable" | "unsupported"
    )
}

/// What a title states: the path the comparison is about, or the root whose
/// changes the list holds.
fn diff_place(view: &WorkspaceDiffView) -> String {
    let path = view.path.trim();
    if !path.is_empty() {
        return path.to_string();
    }
    let root = view.root.trim();
    if root.is_empty() {
        "/".to_string()
    } else {
        root.to_string()
    }
}

/// The patch's own tally: lines it adds and lines it removes.
///
/// The three-character markers are headers rather than content, so they are
/// counted out the way the desktop pane counts them: `+++` and `---` name the
/// files being compared, and a reader's plus count would be off by two for
/// every file in the patch if they were charged.
fn diff_counts(view: &WorkspaceDiffView) -> (usize, usize) {
    let Some(patch) = view.patch.as_deref() else {
        return (0, 0);
    };
    let mut added = 0;
    let mut removed = 0;
    for line in patch.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    (added, removed)
}

/// One change row: the cursor, the status letter the Files tree badges the row
/// with, and the path.
fn diff_row(change: &WorkspaceChangeView, selected: bool, width: usize) -> String {
    let marker = if selected { "› " } else { "  " };
    let letter = match change.letter.trim() {
        "" => "?",
        letter => letter,
    };
    truncate(&format!("{marker}{letter} {}", change.path), width)
}

/// The Diff pane's body: the change list, one patch, or the reason for neither.
fn diff_body(view: &WorkspaceDiffView, selection: usize, width: usize) -> Vec<String> {
    match view.state.as_str() {
        "patch" => {
            let patch = view.patch.as_deref().unwrap_or_default();
            let mut lines: Vec<String> = patch
                .lines()
                .map(|line| truncate(&line.replace('\t', "    "), width))
                .collect();
            if lines.is_empty() {
                return wrap("  ", "this comparison is empty", width);
            }
            if view.truncated {
                lines.push(String::new());
                lines.extend(wrap(
                    "  ",
                    "the comparison continues past what this pane shows",
                    width,
                ));
            }
            lines
        }
        "listed" => view
            .changes
            .iter()
            .enumerate()
            .map(|(index, change)| diff_row(change, index == selection, width))
            .collect(),
        // A clean answer with a path names the file that matches its baseline;
        // with no path it is the whole working copy.
        "clean" => {
            let path = view.path.trim();
            let what = if path.is_empty() {
                "the working copy matches its baseline".to_string()
            } else {
                format!("{path} matches its baseline")
            };
            wrap("  ", &what, width)
        }
        _ => {
            let why = view
                .error
                .as_deref()
                .map(str::trim)
                .filter(|why| !why.is_empty())
                .unwrap_or("the Host could not compare the working copy");
            wrap("  ", why, width)
        }
    }
}

/// The Diff pane's title row: what it compares, its own numbers, and how to
/// leave.
fn diff_title(pane: &DiffPane) -> String {
    let view = &pane.view;
    let place = diff_place(view);
    match view.state.as_str() {
        "patch" => {
            let (added, removed) = diff_counts(view);
            let head = if view.truncated { " · head only" } else { "" };
            format!(
                "diff · {place} · +{added} -{removed}{head} · Up/Down scrolls · Enter list · Esc closes"
            )
        }
        "listed" => format!(
            "diff · {place} · {} changed · Up/Down moves · Enter opens · Esc closes",
            view.changes.len()
        ),
        "clean" => format!("diff · {place} · clean · Esc closes"),
        state => format!("diff · {place} · {state} · Esc closes"),
    }
}

/// The Diff pane: its title, then the body windowed to `rows`.
///
/// A list windows on the cursor the way the Files pane does, and a patch
/// windows on its own offset, so Enter back to the list returns to its top.
fn diff_pane(pane: &DiffPane, width: usize, rows: usize) -> Vec<String> {
    let rows = rows.max(1);
    let body_rows = rows.saturating_sub(1);
    let body = diff_body(&pane.view, pane.selection, width);
    let total = body.len();
    let start = if diff_is_patch(&pane.view) {
        pane.scroll.min(total.saturating_sub(body_rows))
    } else {
        pane.selection
            .saturating_sub(body_rows.saturating_sub(1))
            .min(total.saturating_sub(body_rows))
    };
    let end = (start + body_rows).min(total);
    let mut lines = Vec::with_capacity(rows);
    lines.push(truncate(&diff_title(pane), width));
    lines.extend(body[start..end].iter().cloned());
    lines.resize(rows, String::new());
    lines
}

/// The `/diff` status line: what was compared and how to leave it.
fn diff_summary(view: &WorkspaceDiffView) -> String {
    let place = diff_place(view);
    let vcs = match view.vcs.trim() {
        "" => String::new(),
        vcs => format!(" {vcs}"),
    };
    let mut line = match view.state.as_str() {
        "patch" => {
            let (added, removed) = diff_counts(view);
            let head = if view.truncated { " · head only" } else { "" };
            format!("diff{vcs} · {place} · +{added} -{removed}{head}")
        }
        "listed" => format!("diff{vcs} · {place} · {} changed", view.changes.len()),
        "clean" => format!("diff{vcs} · {place} · clean"),
        state => format!("diff{vcs} · {place} · {state}"),
    };
    if let Some(why) = view
        .error
        .as_deref()
        .map(str::trim)
        .filter(|why| !why.is_empty())
    {
        line.push_str(&format!(" · {why}"));
    }
    line.push_str(" · Esc closes");
    line
}

/// Whether a task board was actually read.
fn tasks_ready(report: &TaskBoardReport) -> bool {
    report.state == "ready"
}

/// The number of rows the board holds across its four sections.
fn tasks_count(report: &TaskBoardReport) -> usize {
    report
        .sections
        .iter()
        .map(|section| section.rows.len())
        .sum()
}

/// The rows one section holds, by the board's own section name.
fn tasks_section_count(report: &TaskBoardReport, name: &str) -> usize {
    report
        .sections
        .iter()
        .filter(|section| section.section == name)
        .map(|section| section.rows.len())
        .sum()
}

/// One task row, the way the desktop board draws it: the label first, then the
/// facts a reader decides on - state, tier, attempts, verification - and the age
/// last, so a column of rows sorts itself by recency at a glance.
///
/// A peer-owned task says so: it is a mirror plus a command channel, not work
/// this node runs, and a reader who took it for local progress would be wrong.
/// Verification is named only when one was requested, because "not requested"
/// on every row would bury the rows that do have a contract.
fn tasks_row(row: &TaskRowView, width: usize, now: DateTime<Utc>) -> String {
    tasks_row_with(row, width, now, &[])
}

/// One row with the edges the graph view adds: the notes a terminal reads where
/// the desktop's canvas would draw a connector, kept inside the row's own width
/// so a long "waits on" list truncates like every other row does.
fn tasks_row_with(row: &TaskRowView, width: usize, now: DateTime<Utc>, notes: &[String]) -> String {
    let title = row.title.trim();
    let title = if title.is_empty() {
        row.id.as_str()
    } else {
        title
    };
    let mut line = format!("• {title} · {}", row.status);
    line.push_str(&format!(" · {}", row.tier));
    if row.host.trim() != "local" && !row.host.trim().is_empty() {
        line.push_str(&format!(" · {}", row.host.trim()));
    }
    line.push_str(&format!(" · {} attempt(s)", row.attempts));
    if row.verification.trim() != "not_requested" && !row.verification.trim().is_empty() {
        line.push_str(&format!(" · verified {}", row.verification.trim()));
    }
    if let Some(fire) = row.next_fire_ms {
        line.push_str(&format!(" · {}", due_in(fire, now)));
    }
    if let Some(created) = DateTime::from_timestamp_millis(row.created_ms as i64) {
        line.push_str(&format!(" · {}", age(created, now)));
    }
    for note in notes {
        line.push_str(&format!(" · {note}"));
    }
    truncate(&line, width)
}

/// When the next fire is, relative to now. A fire whose time has passed says
/// "due" rather than a negative age: the scheduler is about to pick it up.
fn due_in(ms: u64, now: DateTime<Utc>) -> String {
    let Some(at) = DateTime::from_timestamp_millis(ms as i64) else {
        return "due".to_string();
    };
    let seconds = (at - now).num_seconds();
    if seconds <= 0 {
        return "due".to_string();
    }
    match seconds {
        1..=59 => format!("in {seconds}s"),
        60..=3599 => format!("in {}m", seconds / 60),
        3600..=86399 => format!("in {}h", seconds / 3600),
        _ => format!("in {}d", seconds / 86400),
    }
}

/// The Tasks pane's body: one open task's record, or the four board sections.
///
/// A reader who named an id asked for that task, so the record replaces the
/// list until the pane is closed or `/tasks` is typed again - the same
/// narrowing a desktop click on a row makes, without a second pane to dismiss.
fn tasks_body(pane: &TasksPane, width: usize, now: DateTime<Utc>) -> Vec<String> {
    if let Some(detail) = pane.detail.as_ref() {
        return task_detail_body(detail, width, now);
    }
    if pane.graph {
        return tasks_graph_body(&pane.report, width, now);
    }
    tasks_board_body(&pane.report, width, now)
}

/// One task's record, laid out as the desktop's expanded card reads it.
///
/// The intent comes first because it is what the task is for; the facts a row
/// could not fit follow in their own blocks instead of one wide line a terminal
/// would have to truncate. Every block is named, so a block that is absent
/// (no command, no dependencies) is visibly absent rather than an empty row.
fn task_detail_body(detail: &TaskDetailReport, width: usize, now: DateTime<Utc>) -> Vec<String> {
    if detail.state != "ready" {
        let why = detail
            .error
            .as_deref()
            .map(str::trim)
            .filter(|why| !why.is_empty())
            .unwrap_or("the Host holds no record for this task");
        return wrap("  ", why, width);
    }
    let mut lines: Vec<String> = Vec::new();
    let intent = detail.intent.trim();
    if !intent.is_empty() {
        lines.push("intent".to_string());
        lines.extend(indented("  ", intent, width));
    }
    let title = detail.title.trim();
    let name = if title.is_empty() {
        detail.id.trim()
    } else {
        title
    };
    let mut facts = format!(
        "{name} · {} · {} · {}",
        detail.status.trim(),
        detail.tier.trim(),
        detail.host.trim()
    );
    if !detail.parent.trim().is_empty() {
        facts.push_str(&format!(" · parent {}", detail.parent.trim()));
    }
    if !detail.target.trim().is_empty() {
        facts.push_str(&format!(" · target {}", detail.target.trim()));
    }
    lines.push(String::new());
    lines.push("task".to_string());
    lines.extend(wrap("  ", &facts, width));
    lines.push(format!("  id      {}", detail.id.trim()));
    if let Some(created) = DateTime::from_timestamp_millis(detail.created_ms as i64) {
        let mut origin = format!("created {}", age(created, now));
        if let Some(updated) = DateTime::from_timestamp_millis(detail.updated_ms as i64) {
            origin.push_str(&format!(" · updated {}", age(updated, now)));
        }
        lines.extend(wrap("  ", &format!("origin  {origin}"), width));
    }
    let mut trigger = detail.trigger.trim().to_string();
    if let Some(fire) = detail.next_fire_ms {
        trigger.push_str(&format!(" · next {}", due_in(fire, now)));
    }
    if !trigger.trim().is_empty() {
        lines.extend(wrap("  ", &format!("trigger {trigger}"), width));
    }
    if !detail.deps.is_empty() {
        lines.push(String::new());
        lines.push(format!("depends on · {} task(s)", detail.deps.len()));
        lines.extend(wrap("  ", &detail.deps.join(", "), width));
    }
    let command = detail.command.trim();
    if !command.is_empty() {
        lines.push(String::new());
        lines.push("command".to_string());
        lines.extend(indented("  ", command, width));
    }
    lines.push(String::new());
    lines.push("budget".to_string());
    lines.extend(wrap(
        "  ",
        &format!(
            "{} step(s) · {} token(s) · {:.0}s deadline · retry {} up to {} attempt(s)",
            detail.max_steps,
            detail.max_tokens,
            detail.deadline_ms as f64 / 1000.0,
            detail.retry.trim(),
            detail.max_attempts
        ),
        width,
    ));
    lines.push(String::new());
    lines.push("verification".to_string());
    lines.extend(wrap(
        "  ",
        &format!(
            "{} · contract {}",
            detail.verification.trim(),
            detail.verification_contract.trim()
        ),
        width,
    ));
    lines.push(String::new());
    lines.push(format!("attempts · {}", detail.attempts.len()));
    if detail.attempts.is_empty() {
        lines.extend(wrap("  ", "no attempt yet", width));
    }
    for attempt in &detail.attempts {
        lines.push(task_attempt_row(attempt, width, now));
    }
    lines
}

/// One attempt of the open task, the shape the desktop's card prints: the id,
/// the state, when it started, then what it produced.
///
/// A settled attempt carries a result or an error, never both; an attempt that
/// produced a change set names it so a reader knows there is a diff to open with
/// `/review`. The result is cut to a readable length here because the record's
/// bounded result can still be a whole report, and the pane is one screen.
fn task_attempt_row(attempt: &TaskAttemptView, width: usize, now: DateTime<Utc>) -> String {
    let mut line = format!("• {} · {}", attempt.id.trim(), attempt.status.trim());
    if !attempt.tier.trim().is_empty() {
        line.push_str(&format!(" · {}", attempt.tier.trim()));
    }
    if let Some(started) = attempt
        .started_ms
        .and_then(|ms| DateTime::from_timestamp_millis(ms as i64))
    {
        line.push_str(&format!(" · {}", age(started, now)));
    }
    let error = attempt.error.trim();
    let result = attempt.result.trim();
    if !error.is_empty() {
        line.push_str(&format!(" · failed: {}", one_line(error)));
    } else if !result.is_empty() {
        line.push_str(&format!(" · {}", one_line(result)));
    }
    if !attempt.change_set.trim().is_empty() {
        line.push_str(&format!(" · review {}", attempt.change_set.trim()));
    }
    let run = attempt.run.trim();
    if !run.is_empty() {
        line.push_str(&format!(" · run {run}"));
    }
    truncate(&line, width)
}

/// A record's result text squeezed onto one row: attempts print one line each,
/// so a newline in a report does not turn one attempt into twenty rows.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The board body when no task is open: the four sections, each under its own
/// header.
fn tasks_board_body(report: &TaskBoardReport, width: usize, now: DateTime<Utc>) -> Vec<String> {
    if !tasks_ready(report) {
        return tasks_unready_body(report, width);
    }
    if tasks_count(report) == 0 {
        return wrap(
            "  ",
            "the board holds no task yet; a task the model schedules or delegates lands here",
            width,
        );
    }
    let mut lines: Vec<String> = Vec::new();
    for section in &report.sections {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(truncate(
            &format!(
                "{} · {} task(s)",
                section.section.to_uppercase(),
                section.rows.len()
            ),
            width,
        ));
        for row in &section.rows {
            lines.push(tasks_row(row, width, now));
        }
    }
    lines
}

/// Why the board could not be read, as the body explains it.
///
/// One explanation for both views: the board and the graph are two readings of
/// the same report, so they must not differ about why there is nothing to read.
fn tasks_unready_body(report: &TaskBoardReport, width: usize) -> Vec<String> {
    let why = report
        .error
        .as_deref()
        .map(str::trim)
        .filter(|why| !why.is_empty())
        .unwrap_or("this Host carries no task board");
    wrap("  ", why, width)
}

/// The graph's trunk and its tree, as the desktop's graph tab draws them.
///
/// The desktop places a card per durable task under the conversation and draws
/// a connector for each edge. A terminal has neither the canvas nor the curves,
/// so the same edges become the tree's branches, and a dependency a branch
/// cannot show is spelled out on the row that waits on it. The rows are the
/// Host's own, so the graph and the board cannot disagree about what exists.
fn tasks_graph_body(report: &TaskBoardReport, width: usize, now: DateTime<Utc>) -> Vec<String> {
    if !tasks_ready(report) {
        return tasks_unready_body(report, width);
    }
    let rows = tasks_rows(report);
    if rows.is_empty() {
        return wrap(
            "  ",
            "the board holds no task yet; a task the model schedules or delegates lands here",
            width,
        );
    }
    let root = report.root.trim();
    let place = if root.is_empty() {
        "the workspace"
    } else {
        root
    };
    let mut lines = vec![truncate(
        &format!(
            "▸ {place} · {} task(s) · {} edge(s)",
            rows.len(),
            tasks_edge_count(report)
        ),
        width,
    )];
    lines.extend(tasks_tree(&rows, width, now));
    lines
}

/// Every board row, in the order the board lists them: running, queued,
/// scheduled, then history.
fn tasks_rows(report: &TaskBoardReport) -> Vec<&TaskRowView> {
    report
        .sections
        .iter()
        .flat_map(|section| &section.rows)
        .collect()
}

/// How many edges the graph draws: a parent the board holds, plus a dependency
/// on a task the board holds. A name that matches no row is still printed, but
/// it is not an edge this graph can draw.
fn tasks_edge_count(report: &TaskBoardReport) -> usize {
    let rows = tasks_rows(report);
    let known = |id: &str| rows.iter().any(|row| row.id.trim() == id);
    rows.iter()
        .map(|row| {
            let parent = row.parent.trim();
            usize::from(!parent.is_empty() && known(parent))
                + row.deps.iter().filter(|dep| known(dep.trim())).count()
        })
        .sum()
}

/// One row's edges as words: the parent it hangs under, and what it waits on.
///
/// A dep is an edge the tree cannot express - the dependency order is not the
/// parent order - so it travels on the row itself rather than as a branch.
fn tasks_edge_notes(row: &TaskRowView, rows: &[&TaskRowView]) -> Vec<String> {
    let mut notes: Vec<String> = Vec::new();
    let parent = row.parent.trim();
    if !parent.is_empty() && !rows.iter().any(|other| other.id.trim() == parent) {
        notes.push(format!("parent {parent} is off this board"));
    }
    let deps: Vec<&str> = row
        .deps
        .iter()
        .map(|dep| dep.trim())
        .filter(|dep| !dep.is_empty())
        .collect();
    if !deps.is_empty() {
        notes.push(format!("waits on {}", deps.join(", ")));
    }
    notes
}

/// The tree under the trunk: parents before their children, each level hanging
/// off the connectors its ancestors own.
fn tasks_tree(rows: &[&TaskRowView], width: usize, now: DateTime<Utc>) -> Vec<String> {
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); rows.len()];
    let mut roots: Vec<usize> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let parent = row.parent.trim();
        let parent = (!parent.is_empty())
            .then(|| rows.iter().position(|other| other.id.trim() == parent))
            .flatten()
            .filter(|parent| *parent != index);
        match parent {
            Some(parent) => children[parent].push(index),
            // A record can name itself as its parent; that is a row at the
            // trunk, not an edge back into the tree.
            None => roots.push(index),
        }
    }
    let mut tree = TaskTree {
        rows,
        children,
        seen: vec![false; rows.len()],
        folded: 0,
        width,
        now,
        lines: Vec::new(),
    };
    for (position, root) in roots.iter().enumerate() {
        tree.walk(*root, "", position + 1 == roots.len(), 0);
    }
    // Two records naming each other as parent are a cycle, not a tree: every
    // row is still a row the board holds, so a cycle is drawn at the trunk
    // rather than dropped.
    for index in 0..rows.len() {
        if !tree.seen[index] {
            tree.walk(index, "", true, 0);
        }
    }
    if tree.folded > 0 {
        tree.lines.push(truncate(
            &format!("… {} task(s) below the depth cap", tree.folded),
            width,
        ));
    }
    tree.lines
}

/// The walk's own state: the rows, the edges found, and what has been drawn.
struct TaskTree<'a> {
    /// The board's rows, in board order.
    rows: &'a [&'a TaskRowView],
    /// Each row's children, by row index.
    children: Vec<Vec<usize>>,
    /// Whether a row has been drawn.
    seen: Vec<bool>,
    /// Rows the depth cap kept off the tree.
    folded: usize,
    /// The pane's width.
    width: usize,
    /// The moment rows are aged against.
    now: DateTime<Utc>,
    /// The rows drawn so far.
    lines: Vec<String>,
}

impl TaskTree<'_> {
    /// Draw one row and its subtree at `indent`, one level per parent edge.
    ///
    /// The walk keeps an explicit stack of frames rather than recursing: a
    /// parent chain is model-written, and a deep one must cost rows, never the
    /// process stack.
    fn walk(&mut self, index: usize, indent: &str, last: bool, depth: usize) {
        if self.seen[index] {
            return;
        }
        self.seen[index] = true;
        if depth >= GRAPH_DEPTH {
            // The cap folds this row and everything under it into one counted
            // line, so a deep chain costs one row instead of thousands.
            let mut stack = vec![index];
            while let Some(node) = stack.pop() {
                self.folded += 1;
                for child in self.children[node].clone() {
                    if !self.seen[child] {
                        self.seen[child] = true;
                        stack.push(child);
                    }
                }
            }
            return;
        }
        let branch = if last { "└─ " } else { "├─ " };
        let room = self
            .width
            .saturating_sub(cells(indent) + cells(branch))
            .max(24);
        let row = self.rows[index];
        let notes = tasks_edge_notes(row, self.rows);
        let line = tasks_row_with(row, room, self.now, &notes);
        self.lines.push(format!("{indent}{branch}{line}"));
        let next = format!("{indent}{}", if last { "   " } else { "│  " });
        let children = self.children[index].clone();
        for (position, child) in children.iter().enumerate() {
            self.walk(*child, &next, position + 1 == children.len(), depth + 1);
        }
    }
}

/// The Tasks pane's title row: where the board was read, what it holds, and how
/// to leave it.
fn tasks_title(pane: &TasksPane) -> String {
    if let Some(detail) = pane.detail.as_ref() {
        return task_detail_title(detail);
    }
    let report = &pane.report;
    if !tasks_ready(report) {
        return format!("tasks · {} · Esc closes", report.state);
    }
    let root = report.root.trim();
    let place = if root.is_empty() {
        "the workspace"
    } else {
        root
    };
    if pane.graph {
        return format!(
            "tasks graph · {place} · {} task(s) · {} edge(s) · /tasks lists the board · Up/Down scrolls · Esc closes",
            tasks_count(report),
            tasks_edge_count(report)
        );
    }
    format!(
        "tasks · {place} · {} task(s) · {} running · /tasks <id> opens one · Up/Down scrolls · Esc closes",
        tasks_count(report),
        tasks_section_count(report, "running")
    )
}

/// The title of an opened task: which record, its state and where it lives.
fn task_detail_title(detail: &TaskDetailReport) -> String {
    let id = detail.id.trim();
    let id = if id.is_empty() { "no id" } else { id };
    if detail.state != "ready" {
        return format!("task {id} · {} · Esc closes", detail.state.trim());
    }
    let title = detail.title.trim();
    let name = if title.is_empty() {
        String::new()
    } else {
        format!(" · {title}")
    };
    format!(
        "task {id}{name} · {} · {} attempt(s) · /tasks lists the board · Up/Down scrolls · Esc closes",
        detail.status.trim(),
        detail.attempts.len()
    )
}

/// The Tasks pane: its title, then the body windowed to `rows`.
fn tasks_pane(pane: &TasksPane, width: usize, rows: usize) -> Vec<String> {
    let rows = rows.max(1);
    let body_rows = rows.saturating_sub(1);
    let body = tasks_body(pane, width, Utc::now());
    let total = body.len();
    let start = pane.scroll.min(total.saturating_sub(body_rows));
    let end = (start + body_rows).min(total);
    let mut lines = Vec::with_capacity(rows);
    lines.push(truncate(&tasks_title(pane), width));
    lines.extend(body[start..end].iter().cloned());
    lines.resize(rows, String::new());
    lines
}

/// The Trajectory pane's title: which session's log this is, how much it holds,
/// and whether the window follows the live end or sits further back.
fn trajectory_title(pane: &TrajectoryPane, session: Option<&SessionProjection>) -> String {
    let Some(session) = session else {
        return "trajectory · no session · Esc closes".to_string();
    };
    let calls = session.tool_calls.len();
    let events = session.messages.len() + calls;
    let following = if pane.back == 0 {
        "following the newest row"
    } else {
        "scrolled back"
    };
    format!(
        "trajectory · {} · {} turn(s) · {events} event(s) · {calls} call(s) · {following} · Up/Down scrolls · Esc closes",
        session.title.trim(),
        session.executions.len()
    )
}

/// One duration, at the precision a log row needs: milliseconds under a second,
/// tenths under a minute, then minutes and hours. A call that took twelve
/// milliseconds says twelve, not "0.0s".
fn trajectory_duration(ms: u64) -> String {
    if ms < 1000 {
        return format!("{ms}ms");
    }
    let seconds = ms / 1000;
    if seconds < 60 {
        return format!("{}.{}s", seconds, ms % 1000 / 100);
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m {:02}s", seconds % 60);
    }
    format!("{}h {:02}m", minutes / 60, minutes % 60)
}

/// One instant as the row's clock, in the terminal's own zone: a reader
/// comparing these rows to a shell history reads the same wall clock.
fn trajectory_clock(at: DateTime<Utc>) -> String {
    at.with_timezone(&chrono::Local)
        .format("%H:%M:%S")
        .to_string()
}

/// A turn's state, in the word the transcript and the desktop both use for it.
fn execution_word(status: ExecutionStatus) -> &'static str {
    match status {
        ExecutionStatus::Queued => "queued",
        ExecutionStatus::Running => "running",
        ExecutionStatus::WaitingApproval => "waiting",
        ExecutionStatus::Completed => "done",
        ExecutionStatus::Failed => "failed",
        ExecutionStatus::Cancelled => "cancelled",
        ExecutionStatus::Interrupted => "interrupted",
        ExecutionStatus::NeedsIntervention => "needs intervention",
    }
}

/// A turn's header row: its number, when it started, how long it ran and how it
/// stands, with the Host's own failure detail when there is one.
///
/// The span of a turn that has not finished is measured to `now`, so a running
/// turn reads as "8.2s" and grows while the client watches it rather than
/// claiming a duration the Host has not recorded yet.
fn trajectory_turn_row(index: usize, execution: &ExecutionView, end: DateTime<Utc>) -> String {
    let mut line = format!(
        "▸ turn {index} · {} · {} · {}",
        trajectory_clock(execution.started_at),
        trajectory_duration((end - execution.started_at).num_milliseconds().max(0) as u64),
        execution_word(execution.status)
    );
    if let Some(detail) = execution
        .detail
        .as_deref()
        .map(str::trim)
        .filter(|detail| !detail.is_empty())
    {
        line.push_str(&format!(" · {detail}"));
    }
    line
}

/// One event row inside a turn: the clock it happened at, then what it was.
///
/// A message is folded to its first line, because the transcript already draws
/// the whole of it and a log row answers "when did the model speak" rather than
/// repeating the speech. A call carries its step, name, verdict and duration,
/// which is the same record `/audit` shows without the turn grouping.
fn trajectory_event_row(entry: &Entry, width: usize) -> String {
    let line = match entry {
        Entry::Message(message) => {
            let speaker = match (message.compaction, message.role) {
                (true, _) => "checkpoint",
                (false, MessageRole::User) => "you",
                (false, MessageRole::Assistant) => "bsh",
            };
            let head = message.text.lines().next().unwrap_or_default().trim();
            match head.is_empty() {
                true => format!("  {} {speaker}", trajectory_clock(message.at)),
                false => format!("  {} {speaker} · {head}", trajectory_clock(message.at)),
            }
        }
        Entry::Tool(call) => {
            let mut line = format!(
                "  {} #{} {} · {}",
                trajectory_clock(call.started_at),
                call.step,
                call.tool.trim(),
                status_word(call.status)
            );
            if let Some(ms) = call.duration_ms {
                line.push_str(&format!(" · {}", trajectory_duration(ms)));
            }
            let summary = call.summary.trim();
            if !summary.is_empty() {
                line.push_str(&format!(" · {summary}"));
            }
            line
        }
    };
    truncate(&line, width)
}

/// The Trajectory pane's body: one section per turn, oldest first, each holding
/// that turn's messages and calls in the order they were committed.
///
/// Every row is the projection's own record - a message's `at`, a call's
/// `started_at` and `duration_ms`, an execution's span - so a turn cannot show
/// an event the Host did not journal. Events the projection kept without a turn
/// (a message written before the first turn, a call whose execution fell off
/// the projection) are kept under their own heading rather than dropped: a log
/// with a hole in it is worse than a log with an odd section.
fn trajectory_body(
    session: Option<&SessionProjection>,
    width: usize,
    now: DateTime<Utc>,
) -> Vec<String> {
    let Some(session) = session else {
        return wrap("  ", "no session open — Ctrl-T creates one", width);
    };
    let mut executions: Vec<&ExecutionView> = session.executions.iter().collect();
    executions.sort_by_key(|execution| execution.started_at);
    let mut lines: Vec<String> = Vec::new();
    for (index, execution) in executions.iter().enumerate() {
        let end = execution.finished_at.unwrap_or(now);
        lines.push(truncate(
            &trajectory_turn_row(index + 1, execution, end),
            width,
        ));
        let mut entries: Vec<Entry> = session
            .messages
            .iter()
            .filter(|message| message.execution_id == Some(execution.execution_id))
            .map(Entry::Message)
            .chain(
                session
                    .tool_calls
                    .iter()
                    .filter(|call| call.execution_id == Some(execution.execution_id))
                    .map(Entry::Tool),
            )
            .collect();
        entries.sort_by_key(Entry::at);
        for entry in &entries {
            lines.push(trajectory_event_row(entry, width));
        }
    }
    let known = |id: Option<ExecutionId>| {
        id.is_some_and(|id| {
            executions
                .iter()
                .any(|execution| execution.execution_id == id)
        })
    };
    let mut loose: Vec<Entry> = session
        .messages
        .iter()
        .filter(|message| !known(message.execution_id))
        .map(Entry::Message)
        .chain(
            session
                .tool_calls
                .iter()
                .filter(|call| !known(call.execution_id))
                .map(Entry::Tool),
        )
        .collect();
    if !loose.is_empty() {
        loose.sort_by_key(Entry::at);
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(truncate(
            &format!("▸ no turn · {} event(s)", loose.len()),
            width,
        ));
        for entry in &loose {
            lines.push(trajectory_event_row(entry, width));
        }
    }
    if lines.is_empty() {
        return wrap(
            "  ",
            "this session holds no event yet; the first line the reader sends opens a turn",
            width,
        );
    }
    lines
}

/// The Trajectory pane: its title, then the body windowed to `rows`.
///
/// The window is measured from the tail, so `back == 0` shows the newest rows
/// and a reader who scrolled up keeps their place as new events arrive; a body
/// shorter than the pane never scrolls away from its top.
fn trajectory_pane(
    pane: &TrajectoryPane,
    session: Option<&SessionProjection>,
    width: usize,
    rows: usize,
) -> Vec<String> {
    let rows = rows.max(1);
    let body_rows = rows.saturating_sub(1);
    let body = trajectory_body(session, width, Utc::now());
    let total = body.len();
    let max_start = total.saturating_sub(body_rows);
    let start = max_start.saturating_sub(pane.back);
    let end = (start + body_rows).min(total);
    let mut lines = Vec::with_capacity(rows);
    lines.push(truncate(&trajectory_title(pane, session), width));
    lines.extend(body[start..end].iter().cloned());
    lines.resize(rows, String::new());
    lines
}

/// The `/trajectory` status line: the log's numbers and how to leave it.
fn trajectory_summary(session: Option<&SessionProjection>) -> String {
    let Some(session) = session else {
        return "trajectory · no session open — Ctrl-T creates one".to_string();
    };
    format!(
        "trajectory · {} · {} turn(s) · {} event(s) · {} call(s) · newest last · Up/Down scrolls · Esc closes",
        session.title.trim(),
        session.executions.len(),
        session.messages.len() + session.tool_calls.len(),
        session.tool_calls.len()
    )
}

/// The `/tasks <id>` status line: the record the pane opened, or the Host's own
/// reason for opening nothing.
///
/// The refusal is the Host's text, not a second opinion about why a record is
/// missing, so a reader who mistyped an id is told exactly what the store was
/// asked for.
fn task_detail_summary(detail: &TaskDetailReport) -> String {
    let id = detail.id.trim();
    let id = if id.is_empty() { "no id" } else { id };
    if detail.state != "ready" {
        let why = detail
            .error
            .as_deref()
            .map(str::trim)
            .filter(|why| !why.is_empty())
            .unwrap_or("the Host holds no record for this task");
        return format!("task {id} · {} · {why} · Esc closes", detail.state.trim());
    }
    let title = detail.title.trim();
    let name = if title.is_empty() {
        String::new()
    } else {
        format!(" · {title}")
    };
    format!(
        "task {id}{name} · {} · {} attempt(s) · Esc closes · Up/Down scrolls",
        detail.status.trim(),
        detail.attempts.len()
    )
}

/// The `/tasks` status line: the board's numbers and how to leave it.
fn tasks_summary(report: &TaskBoardReport) -> String {
    if !tasks_ready(report) {
        return tasks_unready_status(report, "tasks");
    }
    let mut line = format!(
        "tasks · {} task(s) · {} running · {} queued · {} scheduled · {} settled",
        tasks_count(report),
        tasks_section_count(report, "running"),
        tasks_section_count(report, "queued"),
        tasks_section_count(report, "scheduled"),
        tasks_section_count(report, "history")
    );
    line.push_str(" · Esc closes · Up/Down scrolls");
    line
}

/// The `/tasks graph` status line: the numbers the trunk row carries, and how to
/// leave the view.
fn tasks_graph_summary(report: &TaskBoardReport) -> String {
    if !tasks_ready(report) {
        return tasks_unready_status(report, "tasks graph");
    }
    format!(
        "tasks graph · {} task(s) · {} edge(s) · Esc closes · Up/Down scrolls",
        tasks_count(report),
        tasks_edge_count(report)
    )
}

/// The status line for a board the Host could not read: which view asked, what
/// the query answered, and why.
fn tasks_unready_status(report: &TaskBoardReport, head: &str) -> String {
    let mut line = format!("{head} · {}", report.state);
    if let Some(reason) = report
        .error
        .as_deref()
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
    {
        line.push_str(&format!(" · {reason}"));
    }
    line.push_str(" · Esc closes");
    line
}

/// Whether a federation report read the store.
///
/// `off` still reads the roster - a node can be switched off after its peers
/// trusted it, and the desktop keeps drawing those rows - so only `no_node`,
/// `unsupported` and `unreadable` leave nothing to list.
fn federation_read(report: &FederationReport) -> bool {
    matches!(report.state.as_str(), "ready" | "off")
}

/// How many peers the roster trusts.
fn federation_trusted(report: &FederationReport) -> usize {
    report
        .peers
        .iter()
        .filter(|peer| peer.state == "trusted")
        .count()
}

/// How many capabilities the roster grants in all.
fn federation_grants(report: &FederationReport) -> usize {
    report.peers.iter().map(|peer| peer.grants.len()).sum()
}

/// A node id cut to the prefix the desktop shows beside an enrolled peer's
/// name, so a reader can match the row against an id they pasted elsewhere.
///
/// An unnamed peer keeps its full id instead: it is the only way to tell two
/// unnamed peers apart, and a terminal has no clip that would hide the rest.
fn federation_short(node: &str) -> String {
    node.chars().take(10).collect()
}

/// The name a roster row shows: the note the enrollment carried, else the
/// node id itself.
fn federation_peer_label(peer: &FederationPeerView) -> String {
    match peer
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        Some(name) => format!("{name} · {}", federation_short(peer.node.trim())),
        None => peer.node.trim().to_string(),
    }
}

/// When a grant stops being valid: `expired` once the moment has passed, and
/// the time left before it otherwise.
fn federation_expires(ms: u64, now: DateTime<Utc>) -> String {
    match DateTime::from_timestamp_millis(ms as i64) {
        Some(at) if at <= now => "expired".to_string(),
        Some(_) => format!("ends {}", due_in(ms, now)),
        None => "ends".to_string(),
    }
}

/// A grant's scope list, or `any` when the list is empty: an empty selector
/// permits every member, which is what the desktop's card states.
fn federation_scope(list: &[String]) -> String {
    let parts: Vec<&str> = list
        .iter()
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .collect();
    if parts.is_empty() {
        "any".to_string()
    } else {
        parts.join(",")
    }
}

/// One peer row, the way the desktop's card stack draws it: who the peer is,
/// its trust state, how many capabilities it may invoke, the age of its
/// enrollment and when it lapses.
fn federation_peer_row(peer: &FederationPeerView, width: usize, now: DateTime<Utc>) -> String {
    let mut line = format!("• {} · {}", federation_peer_label(peer), peer.state);
    line.push_str(&format!(" · {} grant(s)", peer.grants.len()));
    if let Some(since) = DateTime::from_timestamp_millis(peer.enrolled_ms as i64) {
        line.push_str(&format!(" · enrolled {}", age(since, now)));
    }
    if let Some(expires) = peer.expires_ms {
        line.push_str(&format!(" · {}", federation_expires(expires, now)));
    }
    truncate(&line, width)
}

/// One capability row under its peer: the capability id and the scope that
/// bounds it, then when it lapses.
fn federation_grant_row(grant: &FederationGrantView, width: usize, now: DateTime<Utc>) -> String {
    let mut line = format!("    · {}", grant.capability);
    line.push_str(&format!(" · ops {}", federation_scope(&grant.operations)));
    line.push_str(&format!(
        " · resources {}",
        federation_scope(&grant.resources)
    ));
    line.push_str(&format!(
        " · workspaces {}",
        federation_scope(&grant.workspaces)
    ));
    if let Some(expires) = grant.expires_ms {
        line.push_str(&format!(" · {}", federation_expires(expires, now)));
    }
    truncate(&line, width)
}

/// The Federation pane's body: the node's hero row, then the peer roster with
/// one row per grant.
///
/// The roster travels even when federation is off or empty, the way the desktop
/// tab keeps its cards: "off" beside a peer's grants is an answer, and a pane
/// that showed nothing would leave a reader unable to tell a switched-off node
/// from an unread store.
fn federation_body(report: &FederationReport, width: usize, now: DateTime<Utc>) -> Vec<String> {
    if !federation_read(report) {
        let why = report
            .error
            .as_deref()
            .map(str::trim)
            .filter(|why| !why.is_empty())
            .unwrap_or("this Host carries no federation node");
        return wrap("  ", why, width);
    }
    let name = report.name.trim();
    let name = if name.is_empty() { "unnamed" } else { name };
    let node = report.node.trim();
    let node = if node.is_empty() {
        "no identity yet"
    } else {
        node
    };
    let state = if report.enabled { "on" } else { "off" };
    let mut hero = format!("NODE · {name} · {node} · {state}");
    if !report.profile.trim().is_empty() {
        hero.push_str(&format!(" · profile {}", report.profile.trim()));
    }
    let mut lines = vec![truncate(&hero, width)];
    if !report.enabled {
        lines.extend(wrap(
            "  ",
            "federation is off in this build's settings; the roster is still read because peers may still trust this node",
            width,
        ));
    }
    if report.peers.is_empty() {
        lines.extend(wrap(
            "  ",
            "no peer is enrolled yet; a peer this node trusts is listed here with the capabilities it may invoke",
            width,
        ));
        return lines;
    }
    lines.push(String::new());
    lines.push(truncate(
        &format!(
            "PEERS · {} peer(s) · {} trusted",
            report.peers.len(),
            federation_trusted(report)
        ),
        width,
    ));
    for peer in &report.peers {
        lines.push(federation_peer_row(peer, width, now));
        for grant in &peer.grants {
            lines.push(federation_grant_row(grant, width, now));
        }
    }
    lines
}

/// The Federation pane's title row: which node the roster belongs to, what it
/// holds, and how to leave it.
fn federation_title(pane: &FederationPane) -> String {
    let report = &pane.report;
    if !federation_read(report) {
        return format!("federation · {} · Esc closes", report.state);
    }
    let name = report.name.trim();
    let name = if name.is_empty() { "this node" } else { name };
    format!(
        "federation · {name} · {} peer(s) · {} trusted · {} grant(s) · Up/Down scrolls · Esc closes",
        report.peers.len(),
        federation_trusted(report),
        federation_grants(report)
    )
}

/// The Federation pane: its title, then the body windowed to `rows`.
fn federation_pane(pane: &FederationPane, width: usize, rows: usize) -> Vec<String> {
    let rows = rows.max(1);
    let body_rows = rows.saturating_sub(1);
    let body = federation_body(&pane.report, width, Utc::now());
    let total = body.len();
    let start = pane.scroll.min(total.saturating_sub(body_rows));
    let end = (start + body_rows).min(total);
    let mut lines = Vec::with_capacity(rows);
    lines.push(truncate(&federation_title(pane), width));
    lines.extend(body[start..end].iter().cloned());
    lines.resize(rows, String::new());
    lines
}

/// The `/federation` status line: the roster's numbers and how to leave it.
fn federation_summary(report: &FederationReport) -> String {
    let mut line = format!(
        "federation · {} · {} peer(s) · {} trusted · {} grant(s)",
        report.state,
        report.peers.len(),
        federation_trusted(report),
        federation_grants(report)
    );
    if let Some(reason) = report
        .error
        .as_deref()
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
    {
        line.push_str(&format!(" · {reason}"));
    }
    line.push_str(" · Esc closes · Up/Down scrolls");
    line
}

/// One child run's lifecycle word, as the roster row and the run's head show
/// it.
///
/// The Host states the state rather than leaving the reader to infer it from a
/// missing answer, so the pane passes the word through; a run whose state came
/// back empty is named as unknown instead of being drawn as finished.
fn subagent_state(run: &SubagentRunView) -> &str {
    let state = run.status.trim();
    if state.is_empty() { "unknown" } else { state }
}

/// The tokens one child run has spent: what it reported, or the estimate the
/// registry keeps while it reports none.
fn subagent_tokens(run: &SubagentRunView) -> u64 {
    let reported = run.prompt_tokens + run.completion_tokens;
    if reported > 0 {
        reported
    } else {
        run.estimated_tokens
    }
}

/// How long one child run took, or how long it has been running.
///
/// A finished run is measured to its finish and a live one to now, so a row a
/// reader watches grows instead of showing the instant it started.
fn subagent_elapsed(run: &SubagentRunView, now: DateTime<Utc>) -> String {
    let end = run
        .finished_at_ms
        .unwrap_or_else(|| now.timestamp_millis().max(0) as u64);
    if end <= run.started_at_ms {
        return "0ms".to_string();
    }
    trajectory_duration(end - run.started_at_ms)
}

/// The roster counted by lifecycle word, as the pane's header says it.
///
/// The desktop's header summarises its child rows the same way: a reader who
/// opened the roster asked how much of it is still working, so running runs lead
/// and the settled ones follow.
fn subagent_counts(runs: &[SubagentRunView]) -> String {
    let mut running = 0usize;
    let mut done = 0usize;
    let mut failed = 0usize;
    for run in runs {
        match subagent_state(run) {
            "running" => running += 1,
            "failed" => failed += 1,
            "done" => done += 1,
            _ => {}
        }
    }
    let mut parts: Vec<String> = Vec::new();
    if running > 0 {
        parts.push(format!("{running} running"));
    }
    if done > 0 {
        parts.push(format!("{done} done"));
    }
    if failed > 0 {
        parts.push(format!("{failed} failed"));
    }
    if parts.is_empty() {
        return "no state yet".to_string();
    }
    parts.join(" · ")
}

/// One roster row: the run's place in the lineage, what it is called, how it
/// stands, and what it has spent.
///
/// The indentation is the desktop graph's: a run nested under a parent is drawn
/// under it, capped so a deep tree cannot push its titles off the frame.
fn subagent_row(
    index: usize,
    run: &SubagentRunView,
    selected: bool,
    width: usize,
    now: DateTime<Utc>,
) -> String {
    let marker = if selected { "› " } else { "  " };
    let indent = "  ".repeat(run.depth.saturating_sub(1).min(3));
    let title = run.title.trim();
    let title = if title.is_empty() {
        "untitled run"
    } else {
        title
    };
    let mut row = format!(
        "{marker}{}. {indent}{title} · {} · {} · {} tok",
        index + 1,
        subagent_state(run),
        subagent_elapsed(run, now),
        crate::state::human_tokens(subagent_tokens(run))
    );
    if run.steps > 0 {
        row.push_str(&format!(" · {} step(s)", run.steps));
    }
    match run.pending {
        0 => {}
        1 => row.push_str(" · 1 queued"),
        queued => row.push_str(&format!(" · {queued} queued")),
    }
    truncate(&row, width)
}

/// One child run as a document: the task it was given and what came back.
///
/// A terminal has no hover card to open on a node and no answer bubble under
/// it, so the run a reader picked is drawn as the task and its answer, the way
/// the detail pane draws a tool call.
fn subagent_detail(run: &SubagentRunView, width: usize, now: DateTime<Utc>) -> Vec<String> {
    let title = run.title.trim();
    let title = if title.is_empty() {
        "untitled run"
    } else {
        title
    };
    let mut lines = vec![truncate(
        &format!(
            "RUN · {title} · {} · {} · {} tok",
            subagent_state(run),
            subagent_elapsed(run, now),
            crate::state::human_tokens(subagent_tokens(run))
        ),
        width,
    )];
    lines.push(truncate(
        &format!(
            "  depth {} · {} step(s) · run {}",
            run.depth,
            run.steps,
            run.id.trim()
        ),
        width,
    ));
    let session = run.session.trim();
    if !session.is_empty() {
        lines.push(truncate(&format!("  conversation {session}"), width));
    }
    lines.push(String::new());
    lines.push(truncate("TASK", width));
    let prompt = run.prompt.trim();
    if prompt.is_empty() {
        lines.extend(wrap("  ", "the model named no task text", width));
    } else {
        lines.extend(wrap("  ", prompt, width));
    }
    lines.push(String::new());
    let error = run
        .error
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    let answer = run
        .result_hint
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    match (error, answer) {
        (Some(error), _) => {
            lines.push(truncate("FAILED", width));
            lines.extend(wrap("  ", error, width));
        }
        (None, Some(answer)) => {
            lines.push(truncate("ANSWER", width));
            lines.extend(wrap("  ", answer, width));
        }
        (None, None) => lines.extend(wrap(
            "  ",
            "no answer yet; a run that is still working reports one when it finishes",
            width,
        )),
    }
    lines
}

/// The child-run pane's body: one run's document, or the roster.
fn subagents_body(pane: &SubagentsPane, width: usize, now: DateTime<Utc>) -> Vec<String> {
    if let Some(run) = pane.open.and_then(|index| pane.report.runs.get(index)) {
        return subagent_detail(run, width, now);
    }
    let report = &pane.report;
    // A build whose executor carries no subagent registry is not a workspace
    // with no delegation: the pane says which of the two it is looking at, so
    // a reader does not hunt for a tool that was never loaded.
    if report.state.trim() != "ready" {
        return wrap(
            "  ",
            &format!(
                "this Host's executor carries no subagent registry ({}), so no turn it ran could delegate",
                report.state.trim()
            ),
            width,
        );
    }
    if report.runs.is_empty() {
        return wrap(
            "  ",
            "no child run yet; a turn that delegates with the subagent tool lists its children here, with the task each was given and the answer it produced",
            width,
        );
    }
    let mut lines = Vec::with_capacity(report.runs.len() + 2);
    lines.push(truncate(
        &format!(
            "RUNS · {} · {}",
            report.runs.len(),
            subagent_counts(&report.runs)
        ),
        width,
    ));
    lines.push(String::new());
    for (index, run) in report.runs.iter().enumerate() {
        lines.push(subagent_row(
            index,
            run,
            index == pane.selection,
            width,
            now,
        ));
    }
    lines
}

/// The child-run pane's title row: what the roster holds and how to walk it.
fn subagents_title(pane: &SubagentsPane) -> String {
    let report = &pane.report;
    if let Some(run) = pane.open.and_then(|index| report.runs.get(index)) {
        let title = run.title.trim();
        let title = if title.is_empty() {
            "untitled run"
        } else {
            title
        };
        return format!(
            "subagents · {title} · {} · Up/Down scrolls · Esc closes",
            subagent_state(run)
        );
    }
    if report.state.trim() != "ready" {
        return format!("subagents · {} · Esc closes", report.state.trim());
    }
    format!(
        "subagents · {} run(s) · {} · Up/Down walks · Enter opens · Esc closes",
        report.runs.len(),
        subagent_counts(&report.runs)
    )
}

/// The child-run pane: its title, then the body windowed to `rows`.
fn subagents_pane(pane: &SubagentsPane, width: usize, rows: usize) -> Vec<String> {
    let rows = rows.max(1);
    let body_rows = rows.saturating_sub(1);
    let body = subagents_body(pane, width, Utc::now());
    let total = body.len();
    let start = pane.scroll.min(total.saturating_sub(body_rows));
    let end = (start + body_rows).min(total);
    let mut lines = Vec::with_capacity(rows);
    lines.push(truncate(&subagents_title(pane), width));
    lines.extend(body[start..end].iter().cloned());
    lines.resize(rows, String::new());
    lines
}

/// The `/subagents` status line: what the roster holds and how to leave it.
fn subagents_summary(report: &SubagentReport, all: bool) -> String {
    let scope = if all {
        "this Host"
    } else {
        "this conversation"
    };
    if report.state.trim() != "ready" {
        return format!(
            "subagents · {} · this build carries no subagent registry",
            report.state.trim()
        );
    }
    format!(
        "subagents · {} run(s) in {scope} · {} · Enter opens · Esc closes",
        report.runs.len(),
        subagent_counts(&report.runs)
    )
}

/// The browser's state in one word, as the pane's title and status line show
/// it.
///
/// The Host states the state rather than leaving the reader to infer it from an
/// empty page: an empty browser, a page in flight and a refused host are three
/// answers, and the desktop pane draws each of them differently.
fn browser_state(view: &BrowserView) -> &str {
    let state = view.status.trim();
    if state == "unsupported" {
        return "unsupported";
    }
    match state {
        "empty" => "empty",
        "loading" => "loading",
        "ready" => "ready",
        "failed" => "failed",
        _ => "empty",
    }
}

/// The address row: where the browser is, and which way it can walk.
///
/// The desktop's address bar carries the URL and the back/forward controls; a
/// terminal has no clickable arrow, so the two directions are stated as words
/// beside the URL instead of drawn as icons that could not be pressed.
fn browser_address(view: &BrowserView) -> String {
    let address = view.address.trim();
    let address = if address.is_empty() {
        "(no address)"
    } else {
        address
    };
    let mut row = format!("address  {address}");
    if view.can_back {
        row.push_str(" · back");
    }
    if view.can_forward {
        row.push_str(" · forward");
    }
    row
}

/// One interactive element as a row: the ref the model would pass back, what
/// it is, its label and the target or value the page gave it.
///
/// The ref leads the row because it is the handle: a reader can see what the
/// turn could click before it clicks it, which is the whole point of the
/// desktop pane listing the elements a page exposes.
fn browser_element_row(element: &BrowserElementView) -> String {
    let kind = element.kind.trim();
    let kind = if kind.is_empty() { "element" } else { kind };
    let label = element.label.trim();
    let mut row = format!("  {} {kind}", element.reference.trim());
    if !label.is_empty() {
        row.push_str(&format!(" \"{label}\""));
    }
    if let Some(input_type) = element
        .input_type
        .as_deref()
        .map(str::trim)
        .filter(|kind| !kind.is_empty())
    {
        row.push_str(&format!(" ({input_type})"));
    }
    if let Some(href) = element.href.as_deref().map(str::trim)
        && !href.is_empty()
    {
        row.push_str(&format!(" → {href}"));
    }
    if let Some(value) = element.value.as_deref().map(str::trim)
        && !value.is_empty()
    {
        row.push_str(&format!(" = {value}"));
    }
    if !element.options.is_empty() {
        row.push_str(&format!(" [{}]", element.options.join(", ")));
    }
    row
}

/// The Browser pane's body: the state line, the page, and the elements under
/// it.
///
/// Every row comes from the Host's view: nothing here fetches a page, so a pane
/// that shows a URL shows it because a turn's browser tool visited it. A cut
/// page says so with its own numbers rather than ending mid-sentence, and a
/// refusal names the rule that blocked the host instead of blaming the network.
fn browser_body(view: &BrowserView, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    lines.push(truncate(&browser_address(view), width));
    if let Some(error) = view
        .error
        .as_deref()
        .map(str::trim)
        .filter(|error| !error.is_empty())
    {
        let rule = view
            .rule
            .as_deref()
            .map(str::trim)
            .filter(|rule| !rule.is_empty())
            .map(|rule| format!(" · rule {rule}"))
            .unwrap_or_default();
        lines.extend(wrap("! ", &format!("{error}{rule}"), width));
    }
    match browser_state(view) {
        // An executor with no browser cannot open a page at all, so the pane
        // says that rather than offering an empty address bar it could not use.
        "unsupported" => return lines,
        "loading" => {
            let address = view.address.trim();
            let address = if address.is_empty() {
                "a page"
            } else {
                address
            };
            lines.push(format!("loading {address} …"));
            return lines;
        }
        "failed" => return lines,
        _ => {}
    }
    let url = view.url.trim();
    if !url.is_empty() {
        lines.push(truncate(&format!("page  {url}"), width));
    }
    if view.text.trim().is_empty() {
        lines.push(String::from(
            "nothing is open · the page a turn's browser tools read appears here",
        ));
        return lines;
    }
    let title = view.title.trim();
    if !title.is_empty() && title != "(untitled)" {
        lines.push(String::from(""));
        lines.push(truncate(title, width));
    }
    lines.push(String::from(""));
    let text: Vec<String> = wrap("  ", &view.text, width).into_iter().collect();
    lines.extend(text);
    if view.text_total_chars > view.text_chars {
        lines.push(truncate(
            &format!(
                "  … {} of {} character(s) shown",
                view.text_chars, view.text_total_chars
            ),
            width,
        ));
    }
    if !view.elements.is_empty() {
        lines.push(String::from(""));
        lines.push(format!(
            "elements · {} of {}",
            view.elements.len(),
            view.elements_total
        ));
        for element in &view.elements {
            lines.push(truncate(&browser_element_row(element), width));
        }
    }
    lines
}

/// The Browser pane's title row: what it shows and how to leave.
fn browser_title(pane: &BrowserPane) -> String {
    let view = &pane.view;
    let state = browser_state(view);
    if state == "unsupported" || state == "empty" || state == "failed" {
        return format!("browser · {state} · Esc closes");
    }
    format!(
        "browser · {state} · {} element(s) · {} char(s) · Up/Down scrolls · Esc closes",
        view.elements_total, view.text_total_chars
    )
}

/// The Browser pane: its title, then the body windowed to `rows`.
///
/// The title stays on the first row while the page scrolls, so a reader who
/// scrolled into a long page still knows what they are reading and how to get
/// back to the transcript.
fn browser_pane(pane: &BrowserPane, width: usize, rows: usize) -> Vec<String> {
    let rows = rows.max(1);
    let body_rows = rows.saturating_sub(1);
    let body = browser_body(&pane.view, width);
    let total = body.len();
    let start = pane.scroll.min(total.saturating_sub(body_rows));
    let end = (start + body_rows).min(total);
    let mut lines = Vec::with_capacity(rows);
    lines.push(truncate(&browser_title(pane), width));
    lines.extend(body[start..end].iter().cloned());
    lines.resize(rows, String::new());
    lines
}

/// The `/browser` status line: what the browser holds and how to leave it.
fn browser_summary(view: &BrowserView) -> String {
    let state = browser_state(view);
    let mut line = format!("browser · {state}");
    let address = view.address.trim();
    if !address.is_empty() {
        line.push_str(&format!(" · {address}"));
    }
    if state == "ready" {
        line.push_str(&format!(
            " · {} char(s) · {} element(s)",
            view.text_total_chars, view.elements_total
        ));
    }
    if let Some(error) = view
        .error
        .as_deref()
        .map(str::trim)
        .filter(|error| !error.is_empty())
    {
        line.push_str(&format!(" · {error}"));
    }
    line.push_str(" · Esc closes · Up/Down scrolls");
    line
}

/// Whether one installed package answers the reader's search text.
///
/// The desktop's search box reads the display name and the id; a terminal
/// reads the contributed names too, because "which package gave me this tool?"
/// is the question a row with twelve tools makes.
fn extension_matches(view: &ExtensionView, filter: &str) -> bool {
    let needle = filter.to_lowercase();
    let haystack = |text: &str| text.to_lowercase().contains(&needle);
    haystack(&view.id)
        || haystack(&view.name)
        || view.tools.iter().any(|tool| haystack(tool))
        || view.commands.iter().any(|command| haystack(command))
}

/// One installed package's rows: what the desktop's list names it, then what
/// it contributes and why it is or is not in force.
///
/// A desktop card carries the version, the state and the contributions; a
/// terminal row keeps the same facts in three lines that answer "is this on?"
/// before "what does it give me?". A package that is switched on but not
/// applied is stated as such: that pair is the one a reader has to act on, and
/// an empty state would read as healthy.
fn extension_row(view: &ExtensionView, width: usize) -> Vec<String> {
    let mut head = format!("• {}", view.name.trim());
    if !view.version.trim().is_empty() {
        head.push_str(&format!(" · {}", view.version.trim()));
    }
    if !view.state.trim().is_empty() {
        head.push_str(&format!(" · {}", view.state.trim()));
    }
    if !view.health.trim().is_empty() {
        head.push_str(&format!(" · {}", view.health.trim()));
    }
    if view.configured && !view.applied {
        head.push_str(" · configured, not applied");
    } else if !view.configured {
        head.push_str(" · switched off");
    }
    let mut rows = vec![truncate(&head, width)];
    let mut meta = format!("  {} · {}", view.runtime, view.trust);
    if !view.signature.trim().is_empty() {
        meta.push_str(&format!(" · {}", view.signature.trim()));
    }
    if !view.source.trim().is_empty() {
        meta.push_str(&format!(" · {}", view.source.trim()));
    }
    // The id is the row's key: a reader who installed two versions of one
    // package needs the exact id to tell them apart.
    if view.id.trim() != view.name.trim() {
        meta.push_str(&format!(" · id {}", view.id.trim()));
    }
    rows.push(truncate(&meta, width));
    let mut contributed: Vec<String> = Vec::new();
    if !view.tools.is_empty() {
        contributed.push(format!("tools: {}", view.tools.join(", ")));
    }
    if !view.commands.is_empty() {
        contributed.push(format!("commands: {}", view.commands.join(", ")));
    }
    if !view.panels.is_empty() {
        contributed.push(format!("panels: {}", view.panels.join(", ")));
    }
    if contributed.is_empty() {
        contributed.push("contributes nothing this Host can call".to_string());
    }
    rows.push(truncate(&format!("  {}", contributed.join(" · ")), width));
    // The failure is the row a reader cannot get anywhere else: a package that
    // did not load still lists its contributions, and this line says why none
    // of them is in force.
    if let Some(error) = view
        .error
        .as_deref()
        .map(str::trim)
        .filter(|error| !error.is_empty())
    {
        rows.extend(wrap("  ! ", error, width));
    }
    rows
}

/// The Extensions pane's body: the roster's state, then one block per package.
///
/// A roster that could not be read is stated with its reason rather than
/// drawn as an empty list: "no package is installed" and "the host did not
/// answer" are different answers, and only the second one needs an operator.
fn extensions_body(report: &ExtensionsReport, filter: Option<&str>, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    if report.state != "ready" {
        lines.extend(wrap(
            "! ",
            &format!(
                "the extension roster could not be read: {}",
                report.error.as_deref().unwrap_or("no detail")
            ),
            width,
        ));
        return lines;
    }
    let filter = filter.map(str::trim).filter(|filter| !filter.is_empty());
    let matched: Vec<&ExtensionView> = report
        .extensions
        .iter()
        .filter(|view| filter.is_none_or(|filter| extension_matches(view, filter)))
        .collect();
    if matched.is_empty() {
        match filter {
            Some(filter) => lines.extend(wrap(
                "  ",
                &format!("no extension matched \"{filter}\""),
                width,
            )),
            None => lines.extend(wrap("  ", "no extension is installed here", width)),
        }
        return lines;
    }
    for view in matched {
        lines.extend(extension_row(view, width));
    }
    lines
}

/// The Extensions pane's title row: what it shows and how to leave.
fn extensions_title(pane: &ExtensionsPane) -> String {
    let total = pane.report.extensions.len();
    let filter = pane
        .filter
        .as_deref()
        .map(str::trim)
        .filter(|filter| !filter.is_empty());
    let shown = pane
        .report
        .extensions
        .iter()
        .filter(|view| filter.is_none_or(|filter| extension_matches(view, filter)))
        .count();
    match filter {
        Some(filter) => format!(
            "extensions · \"{filter}\" · {shown} of {total} package(s) · Up/Down scrolls · Esc closes"
        ),
        None => format!("extensions · {total} package(s) · Up/Down scrolls · Esc closes"),
    }
}

/// The Extensions pane: its title, then the body windowed to `rows`.
///
/// The title stays on the first row while the body scrolls, so a reader who
/// scrolled into a long roster still knows what they are reading and how to
/// get back to the transcript.
fn extensions_pane(pane: &ExtensionsPane, width: usize, rows: usize) -> Vec<String> {
    let rows = rows.max(1);
    let body_rows = rows.saturating_sub(1);
    let body = extensions_body(&pane.report, pane.filter.as_deref(), width);
    let total = body.len();
    let start = pane.scroll.min(total.saturating_sub(body_rows));
    let end = (start + body_rows).min(total);
    let mut lines = Vec::with_capacity(rows);
    lines.push(truncate(&extensions_title(pane), width));
    lines.extend(body[start..end].iter().cloned());
    lines.resize(rows, String::new());
    lines
}

/// The Review pane: its title, then the body windowed to `rows`.
///
/// The title stays on the first row while the body moves, so a reader who
/// scrolled to the end of a diff still knows whose diff it is.
fn review_pane(pane: &ReviewPane, width: usize, rows: usize) -> Vec<String> {
    let rows = rows.max(1);
    let body_rows = rows.saturating_sub(1);
    let (body, focus) = review_body(pane, width);
    let total = body.len();
    // A diff opens at its top and moves only when the reader scrolls; the list
    // follows its highlight, so walking the rows never leaves the selected
    // change off screen - the desktop's list scrolls to its selected row for
    // the same reason.
    let start = match focus {
        Some(focus) => focus
            .saturating_sub(body_rows.saturating_sub(1))
            .min(total.saturating_sub(body_rows)),
        None => pane.scroll.min(total.saturating_sub(body_rows)),
    };
    let end = (start + body_rows).min(total);
    let mut lines = Vec::with_capacity(rows);
    lines.push(truncate(&review_title(pane), width));
    lines.extend(body[start..end].iter().cloned());
    lines.resize(rows, String::new());
    lines
}

/// The Review pane's body, with the line its highlight sits on in list mode.
///
/// A report that named a change carries that change's diff instead of the
/// list: the desktop draws the two side by side, and one terminal column shows
/// them in turn rather than half a diff.
fn review_body(pane: &ReviewPane, width: usize) -> (Vec<String>, Option<usize>) {
    let report = &pane.report;
    if let Some(diff) = &report.diff {
        return (review_diff_rows(diff, width), None);
    }
    let mut lines = review_head(report, width);
    if report.state != "ready" {
        let reason = report
            .error
            .as_deref()
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .unwrap_or("no detail");
        lines.extend(wrap(
            "  ",
            &format!("there is no changeset to read: {reason}"),
            width,
        ));
        return (lines, None);
    }
    if report.changes.is_empty() {
        lines.extend(wrap("  ", "this changeset changed no file", width));
        return (lines, None);
    }
    let selected = pane.selection.min(report.changes.len() - 1);
    let mut focus = None;
    for (index, change) in report.changes.iter().enumerate() {
        if index == selected {
            focus = Some(lines.len());
        }
        lines.extend(review_change_rows(change, index == selected, width));
    }
    lines.extend(review_findings_rows(report, width));
    lines.extend(review_verifications_rows(report, width));
    lines.extend(review_decisions_rows(report, width));
    (lines, focus)
}

/// The rows above the change list: what the changeset is and how it read.
///
/// These are the chips the desktop draws across its Review header - status,
/// risk, capture and digest - plus the turn that produced the changeset, kept
/// on their own rows so a narrow terminal never loses one to truncation.
fn review_head(report: &ReviewReport, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    if report.state != "ready" {
        return lines;
    }
    let mut facts = String::from("  changeset ");
    facts.push_str(&report.change_set.as_deref().map(short).unwrap_or_default());
    if let Some(turn) = report.turn.as_deref() {
        facts.push_str(&format!(" · turn {turn}"));
    }
    if let Some(task) = report.task.as_deref() {
        facts.push_str(&format!(" · task {task}"));
    }
    lines.push(truncate(&facts, width));
    let mut capture = format!(
        "  {} · captured {}",
        review_status_word(report),
        report.capture
    );
    if let Some(digest) = report.digest.as_deref() {
        capture.push_str(&format!(" · digest {}", short(digest)));
    }
    lines.push(truncate(&capture, width));
    let mut said = format!("  review {} · risk {}", report.risk, report.status);
    if report.requires_human {
        said.push_str(" · needs a human decision");
    }
    lines.push(truncate(&said, width));
    if let Some(error) = report
        .error
        .as_deref()
        .map(str::trim)
        .filter(|error| !error.is_empty())
    {
        lines.extend(wrap("  ! ", error, width));
    }
    lines
}

/// The word for the changeset's lifecycle, with the capture's own caveats: a
/// partial capture is stated where a reader looks for the list, not only deep
/// in the record.
fn review_status_word(report: &ReviewReport) -> String {
    if report.changeset_status.is_empty() {
        return "changeset".to_string();
    }
    report.changeset_status.clone()
}

/// One change row, and the detail rows that explain it.
///
/// The desktop's row carries the path, the net effect, the tools that touched
/// it and its revision count; a terminal row carries the same fields in one
/// line, and the highlighted row is marked the way its lists mark a selection.
fn review_change_rows(change: &ReviewChangeView, selected: bool, width: usize) -> Vec<String> {
    let marker = if selected { "› " } else { "  " };
    let tool_label = match change.tools.first() {
        Some(tool) if change.tools.len() > 1 => format!("{tool} +{}", change.tools.len() - 1),
        Some(tool) => tool.clone(),
        None => "no tool".to_string(),
    };
    let mut row = format!(
        "{marker}{} · {} · {} rev · {tool_label}",
        change.path, change.kind, change.revisions
    );
    if change.partial {
        row.push_str(" · partial capture");
    }
    let mut rows = vec![truncate(&row, width)];
    if let Some(from) = change.from.as_deref() {
        rows.push(truncate(&format!("    from {from}"), width));
    }
    rows
}

/// The deterministic findings attached to the changeset.
///
/// They are the review's own rows, not a client's guess: the desk's risk level
/// comes from them, so a reader who sees `risk high` can read which finding
/// said so.
fn review_findings_rows(report: &ReviewReport, width: usize) -> Vec<String> {
    if report.findings.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![truncate(
        &format!("  findings · {} row(s)", report.findings.len()),
        width,
    )];
    for finding in &report.findings {
        let mut row = format!(
            "    {} · {} · {}",
            finding.severity, finding.kind, finding.status
        );
        if !finding.title.trim().is_empty() {
            row.push_str(&format!(" · {}", finding.title));
        }
        lines.push(truncate(&row, width));
        if !finding.detail.trim().is_empty() {
            lines.extend(wrap("      ", finding.detail.trim(), width));
        }
    }
    lines
}

/// Every verification recorded against this review, oldest first: the command
/// that ran, its exit code and its one-line summary - the desktop panel's
/// verification block, row for row.
fn review_verifications_rows(report: &ReviewReport, width: usize) -> Vec<String> {
    if report.verifications.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![truncate(
        &format!("  verifications · {} row(s)", report.verifications.len()),
        width,
    )];
    for verification in &report.verifications {
        let mut row = format!("    {} · {}", verification.kind, verification.status);
        if let Some(command) = verification.command.as_deref() {
            row.push_str(&format!(" · {command}"));
        }
        if let Some(code) = verification.exit_code {
            row.push_str(&format!(" · exit {code}"));
        }
        lines.push(truncate(&row, width));
        if !verification.summary.trim().is_empty() {
            lines.extend(wrap("      ", verification.summary.trim(), width));
        }
    }
    lines
}

/// The decisions already recorded on this review, oldest first, so a reader
/// sees whether a human approved the change the list is showing.
fn review_decisions_rows(report: &ReviewReport, width: usize) -> Vec<String> {
    if report.decisions.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![truncate(
        &format!("  decisions · {} row(s)", report.decisions.len()),
        width,
    )];
    for decision in &report.decisions {
        let mut row = format!("    {} by {}", decision.decision, decision.actor);
        if decision.digest.chars().count() > 8 {
            row.push_str(&format!(" · digest {}", short(&decision.digest)));
        }
        if !decision.rationale.trim().is_empty() {
            row.push_str(&format!(" · {}", decision.rationale.trim()));
        }
        lines.push(truncate(&row, width));
    }
    lines
}

/// One change's prepared diff: its net effect, the definitions it moved and the
/// unified text rows.
///
/// The rows are the ones the desktop's diff pane draws - the same prepared
/// comparison, so a hunk reads at the same place in both interfaces. The
/// structural block comes first because a reader asked for this file to see
/// what it did to its code, not only which lines moved.
fn review_diff_rows(diff: &ReviewDiffView, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(truncate(
        &format!(
            "  {} · {} · +{} -{} · {} → {} bytes",
            diff.path, diff.kind, diff.added, diff.removed, diff.old_bytes, diff.new_bytes
        ),
        width,
    ));
    if diff.binary {
        lines.extend(wrap("  ", "binary file: there is no text diff", width));
    }
    if let Some(note) = diff
        .note
        .as_deref()
        .map(str::trim)
        .filter(|note| !note.is_empty())
    {
        lines.extend(wrap("  ! ", note, width));
    }
    if !diff.structural.is_empty() {
        lines.push(truncate(
            &format!("  structural · {} node(s)", diff.structural.len()),
            width,
        ));
        for node in &diff.structural {
            let mut row = format!("    {} {} · {}", node.change, node.kind, node.name);
            match (node.old_line, node.new_line) {
                (Some(old), Some(new)) => row.push_str(&format!(" · {old} → {new}")),
                (None, Some(new)) => row.push_str(&format!(" · line {new}")),
                (Some(old), None) => row.push_str(&format!(" · was line {old}")),
                (None, None) => {}
            }
            if !node.detail.trim().is_empty() {
                row.push_str(&format!(" · {}", node.detail.trim()));
            }
            lines.push(truncate(&row, width));
        }
    }
    if diff.lines.is_empty() {
        if !diff.binary {
            lines.extend(wrap("  ", "this change has no text rows", width));
        }
        return lines;
    }
    for line in &diff.lines {
        // One gutter cell of mark and one of the number the row sits at in the
        // file it now belongs to: the old line for a removal, the new one
        // otherwise. The desktop's pane labels the same side.
        let number = match line.mark.as_str() {
            "-" => line.old_line,
            _ => line.new_line.or(line.old_line),
        };
        let gutter = number
            .map(|number| format!("{number:>4}"))
            .unwrap_or_default();
        lines.push(truncate(
            &format!("{} {} {}", line.mark, gutter, line.text),
            width,
        ));
    }
    lines
}

/// The Review pane's title row: what it shows and how to leave.
///
/// The list offers the gestures it accepts - walk the rows and open one - and
/// a diff offers only the way back, because a diff has no rows to walk.
fn review_title(pane: &ReviewPane) -> String {
    let report = &pane.report;
    if let Some(diff) = &report.diff {
        return format!(
            "review · {} · +{} -{} · Up/Down scrolls · Esc goes back",
            diff.path, diff.added, diff.removed
        );
    }
    if report.state != "ready" {
        return format!("review · {} · Esc closes", report.state);
    }
    let mut title = format!(
        "review · {} change(s) · {} · risk {}",
        report.changes.len(),
        review_status_word(report),
        report.risk
    );
    if report.requires_human {
        title.push_str(" · needs a human");
    }
    title.push_str(" · Up/Down walks · Enter opens · Esc closes");
    title
}

/// The `/review` status line: the changeset it read, or why it could not.
fn review_summary(report: &ReviewReport, change: Option<&str>) -> String {
    if report.state != "ready" {
        let mut line = format!("review · {}", report.state);
        if let Some(reason) = report
            .error
            .as_deref()
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
        {
            line.push_str(&format!(" · {reason}"));
        }
        line.push_str(" · Esc closes");
        return line;
    }
    if let Some(diff) = &report.diff {
        return format!(
            "review · {} · {} · +{} -{} · Esc goes back",
            diff.path, diff.kind, diff.added, diff.removed
        );
    }
    let mut line = match change {
        Some(named) => format!(
            "review · '{named}' names no change in this changeset · {} change(s) listed",
            report.changes.len()
        ),
        None => format!(
            "review · {} change(s) · {} · risk {} · Enter opens one · Esc closes",
            report.changes.len(),
            review_status_word(report),
            report.risk
        ),
    };
    if change.is_some()
        && let Some(reason) = report
            .error
            .as_deref()
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
    {
        line = format!("review · {reason}");
    }
    line
}

/// The `/extensions` status line: how many packages are in force, and how many
/// the Host reported a failure for.
fn extensions_summary(report: &ExtensionsReport, filter: Option<&str>) -> String {
    if report.state != "ready" {
        let mut line = format!("extensions · {}", report.state);
        if let Some(reason) = report
            .error
            .as_deref()
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
        {
            line.push_str(&format!(" · {reason}"));
        }
        line.push_str(" · Esc closes");
        return line;
    }
    let total = report.extensions.len();
    let applied = report.extensions.iter().filter(|view| view.applied).count();
    let failed = report
        .extensions
        .iter()
        .filter(|view| {
            view.state == "failed"
                || view
                    .error
                    .as_deref()
                    .is_some_and(|error| !error.trim().is_empty())
        })
        .count();
    let mut line = match (filter, total) {
        (Some(filter), _) => format!("extensions · \"{filter}\" · {total} package(s)"),
        (None, 0) => "extensions · no package is installed here".to_string(),
        (None, total) => format!("extensions · {total} package(s)"),
    };
    if total > 0 {
        line.push_str(&format!(" · {applied} applied"));
        if failed > 0 {
            line.push_str(&format!(" · {failed} failed"));
        }
    }
    line.push_str(" · Esc closes · Up/Down scrolls");
    line
}

/// The word the terminal prints for a call's family: the desktop AUDIT page's
/// own categories, so the two surfaces name a call the same way.
fn audit_category_word(category: crate::audit::AuditCategory) -> &'static str {
    use crate::audit::AuditCategory;
    match category {
        AuditCategory::FileRead => "read",
        AuditCategory::FileWrite => "write",
        AuditCategory::Shell => "shell",
        AuditCategory::Network => "network",
        AuditCategory::Browser => "browser",
        AuditCategory::Mcp => "mcp",
        AuditCategory::Subagent => "subagent",
        AuditCategory::Other => "other",
    }
}

/// Where one call stands in the audit's vocabulary. A call with no result yet
/// is pending, which is not the same answer as a call that failed.
fn audit_outcome_word(status: ToolCallStatus) -> &'static str {
    match status {
        ToolCallStatus::Running => "pending",
        ToolCallStatus::Completed => "ok",
        ToolCallStatus::Failed => "failed",
    }
}

/// The 1-based turn a call belongs to, when the projection still holds the
/// execution that made it.
fn audit_turn_of(session: &SessionProjection, call: &ToolCallView) -> Option<usize> {
    let id = call.execution_id?;
    session
        .executions
        .iter()
        .position(|execution| execution.execution_id == id)
        .map(|index| index + 1)
}

/// Whether a call answers the pane's filter: its tool, target, category or
/// outcome, matched without case. Free text rather than a query language, so a
/// path with spaces is one filter like any other.
fn audit_matches(call: &ToolCallView, filter: &str) -> bool {
    let filter = filter.to_lowercase();
    let category = audit_category_word(crate::audit::AuditCategory::classify(&call.tool));
    let target = crate::audit::target_of(&call.args);
    let outcome = audit_outcome_word(call.status);
    call.tool.to_lowercase().contains(&filter)
        || target.to_lowercase().contains(&filter)
        || category.contains(&filter)
        || outcome.contains(&filter)
}

/// One audited call: the tool and what it touched, then its family, outcome and
/// cost on the same line, with the bounded preview beneath it.
///
/// The preview is the audit module's own redacted, flattened cut - the terminal
/// does not render an unbounded output or invent a second redaction rule for
/// it.
fn audit_row(session: &SessionProjection, call: &ToolCallView, width: usize) -> Vec<String> {
    let category = audit_category_word(crate::audit::AuditCategory::classify(&call.tool));
    let target = crate::audit::target_of(&call.args);
    let mut line = String::from("• ");
    if let Some(turn) = audit_turn_of(session, call) {
        line.push_str(&format!("T{turn} "));
    }
    line.push_str(call.tool.trim());
    if !target.is_empty() {
        line.push(' ');
        line.push_str(&target);
    }
    line.push_str(&format!(
        " · {category} · {}",
        audit_outcome_word(call.status)
    ));
    if let Some(ms) = call.duration_ms {
        line.push_str(&format!(" · {ms}ms"));
    }
    let mut rows = vec![truncate(&line, width)];
    let preview = crate::audit::preview(
        call.output
            .as_deref()
            .filter(|output| !output.trim().is_empty())
            .unwrap_or(&call.args),
    );
    if !preview.is_empty() {
        rows.extend(wrap("  ", &preview, width));
    }
    rows
}

/// The Audit pane's body: every call the projection holds, the filter's answer,
/// and the approvals an operator refused.
///
/// The page is derived exactly the way the desktop's is - from the session's
/// own tool-call rows - so a call the transcript shows cannot be missing here.
fn audit_body(
    session: Option<&SessionProjection>,
    filter: Option<&str>,
    width: usize,
) -> Vec<String> {
    let Some(session) = session else {
        return wrap("  ", "no session open — Ctrl-T creates one", width);
    };
    let mut lines: Vec<String> = Vec::new();
    let mut matched = 0usize;
    for call in &session.tool_calls {
        if filter.is_some_and(|filter| !audit_matches(call, filter)) {
            continue;
        }
        matched += 1;
        lines.extend(audit_row(session, call, width));
    }
    if matched == 0 {
        match filter {
            Some(filter) => lines.extend(wrap(
                "  ",
                &format!("no audited call matched \"{filter}\""),
                width,
            )),
            None => lines.extend(wrap("  ", "this session has made no tool call yet", width)),
        }
    }
    // What was refused is half the page's question: a call that an operator
    // rejected never reached the transcript's tool rows, so the approval is the
    // only record of it.
    let refused: Vec<&ApprovalView> = session
        .approvals
        .iter()
        .filter(|approval| approval.status == ApprovalStatus::Rejected)
        .collect();
    if !refused.is_empty() {
        lines.push(String::new());
        lines.push(truncate(&format!("refused · {}", refused.len()), width));
        for approval in refused {
            let resource = approval
                .action
                .resource
                .as_deref()
                .unwrap_or(&approval.summary);
            lines.push(truncate(
                &format!(
                    "✕ {} {} · {}",
                    approval.action.tool.trim(),
                    approval.action.action.trim(),
                    resource.trim()
                ),
                width,
            ));
        }
    }
    lines
}

/// The Audit pane's title row: what it shows and how to leave.
fn audit_title(pane: &AuditPane, session: Option<&SessionProjection>) -> String {
    let total = session.map_or(0, |session| session.tool_calls.len());
    match pane.filter.as_deref() {
        Some(filter) => {
            format!("audit · filter \"{filter}\" · {total} call(s) · Up/Down scrolls · Esc closes")
        }
        None => format!("audit · {total} call(s) · Up/Down scrolls · Esc closes"),
    }
}

/// The Audit pane: its title, then the body windowed to `rows`.
///
/// The title stays on the first row while the body scrolls, so a reader who
/// scrolled into a long conversation still knows what they are reading and how
/// to get back to the transcript.
fn audit_pane(
    pane: &AuditPane,
    session: Option<&SessionProjection>,
    width: usize,
    rows: usize,
) -> Vec<String> {
    let rows = rows.max(1);
    let body_rows = rows.saturating_sub(1);
    let body = audit_body(session, pane.filter.as_deref(), width);
    let total = body.len();
    let start = pane.scroll.min(total.saturating_sub(body_rows));
    let end = (start + body_rows).min(total);
    let mut lines = Vec::with_capacity(rows);
    lines.push(truncate(&audit_title(pane, session), width));
    lines.extend(body[start..end].iter().cloned());
    lines.resize(rows, String::new());
    lines
}

/// The `/audit` status line: how many calls the page holds, how many were
/// refused, and what the filter answered.
fn audit_summary(session: &SessionProjection, filter: Option<&str>) -> String {
    let calls = session.tool_calls.len();
    let turns = session.executions.len();
    let refused = session
        .approvals
        .iter()
        .filter(|approval| approval.status == ApprovalStatus::Rejected)
        .count();
    let mut line = match filter {
        Some(filter) => {
            let hits = session
                .tool_calls
                .iter()
                .filter(|call| audit_matches(call, filter))
                .count();
            format!("audit · {hits} of {calls} call(s) match \"{filter}\"")
        }
        None => format!("audit · {calls} call(s) in {turns} turn(s)"),
    };
    if refused > 0 {
        line.push_str(&format!(" · {refused} refused"));
    }
    line.push_str(" · Esc closes · Up/Down scrolls");
    line
}

/// The card an empty session opens on: the desktop's hero, as terminal rows.
///
/// A window opens on a hero that says where the reader is - the folder the
/// Host serves, the model the open turn will resolve from, and the two
/// gestures the composer answers - so an empty terminal draws the same three
/// facts inside the transcript's own box instead of one grey sentence. The
/// rows reuse the fence markers, so the skin paints the card without a new
/// vocabulary, and every row is cut to the frame's width.
fn hero_rows(
    session: Option<&SessionProjection>,
    workspace: Option<&Path>,
    width: usize,
) -> Vec<String> {
    // The title is the folder's own name, the way the desktop hero and its
    // workspace rail open. A Host serving no workspace says so rather than
    // letting the reader guess which folder a turn would run in.
    let folder = match workspace.and_then(Path::file_name) {
        Some(name) => name.to_string_lossy().into_owned(),
        None => "no workspace".to_string(),
    };
    let mut rows = vec![format!("╭ {folder}")];
    match session {
        Some(session) => {
            // The open conversation, then what a turn here resolves from: the
            // same `config_summary` the header chip and `/settings` read.
            rows.extend(wrap("│ ", &session.title, width));
            rows.extend(wrap("│ ", &config_summary(&session.config), width));
        }
        // Nothing to resolve yet: the Host answers the catalogue before it
        // answers a session list, so the card names the key that starts one.
        None => rows.extend(wrap(
            "│ ",
            "no session open · ^T starts one · ^N/^P walk them",
            width,
        )),
    }
    rows.push("│".to_string());
    rows.extend(wrap(
        "│ ",
        "Enter sends a turn · / lists commands · @ mentions a file",
        width,
    ));
    rows.push("╰".to_string());
    rows.into_iter().map(|row| truncate(&row, width)).collect()
}

fn config_summary(config: &SessionConfig) -> String {
    let model = config
        .model_ref()
        .map(|(provider, model)| format!("{provider}/{model}"))
        .unwrap_or_else(|| "default model".to_string());
    let preset = config
        .preset
        .clone()
        .unwrap_or_else(|| "default preset".to_string());
    let plan = if config.plan_mode { " · plan on" } else { "" };
    format!("{model} · {preset} · {}{plan}", config.permission)
}

/// The session's token buckets, summed from the turns that reported one.
///
/// The desktop's usage pill sums its chat's turns; the Host journals each
/// turn's figures onto the assistant message it committed, so a client adds
/// the same rows rather than keeping a counter that a reload would lose. A
/// provider that reports more cached tokens than prompt tokens is reporting
/// nonsense, and the desktop's own sum clamps that rather than carrying it.
fn session_usage(session: &SessionProjection) -> Option<TurnUsage> {
    let mut total = TurnUsage::default();
    for usage in session.messages.iter().filter_map(|message| message.usage) {
        total.prompt_tokens += usage.prompt_tokens;
        total.completion_tokens += usage.completion_tokens;
        total.cached_tokens += usage.cached_tokens.min(usage.prompt_tokens);
    }
    (!total.is_empty()).then_some(total)
}

/// The header's usage chip: the desktop's pill in one fragment.
///
/// It shows the compact total the pill shows, plus the cache share when the
/// provider reported one; `/usage` is where the exact figures are.
fn usage_chip(usage: &TurnUsage) -> String {
    let total = crate::state::human_tokens(usage.total());
    match crate::state::cache_hit_percent(usage.cached_tokens, usage.prompt_tokens) {
        Some(percent) => format!("usage {total} · {percent}% cached"),
        None => format!("usage {total}"),
    }
}

/// The answer `/copy` puts on the clipboard: the `from_end`-th message the
/// model wrote, counting the newest as one, or `None` when the session holds
/// fewer answers than that.
///
/// The reader's own turns and compaction checkpoints are skipped: both are
/// rows the transcript shows, but what a reader copies out of a conversation
/// is what the model answered. A checkpoint is condensed history the Host
/// wrote, so copying it would read as an answer nobody gave.
fn copy_text(session: &SessionProjection, from_end: usize) -> Option<&str> {
    session
        .messages
        .iter()
        .rev()
        .filter(|message| !message.compaction && message.role == MessageRole::Assistant)
        .nth(from_end.checked_sub(1)?)
        .map(|message| message.text.as_str())
        .filter(|text| !text.trim().is_empty())
}

/// At most ten entries of a listing, marked when more exist.
/// The command a picker's choice commits.
///
/// A pick becomes the line the reader could have typed: the two gestures end at
/// the same `SlashCommand`, so a chosen model can never take a different path
/// to the Host than a named one.
fn pick_command(choice: picker::Choice) -> SlashCommand {
    match choice {
        picker::Choice::Model { provider, model } => {
            SlashCommand::Model(Some(provider), Some(model))
        }
        picker::Choice::Preset(preset) => SlashCommand::Preset(Some(preset)),
        picker::Choice::Permission(permission) => SlashCommand::Permission(Some(permission)),
        // The listing numbers its rows from one, so the pick carries that
        // number: `/sessions 2` is what the same click on the printed list is.
        picker::Choice::Session(index) => SlashCommand::Sessions(Some((index + 1).to_string())),
        // The bin's rows are numbered the way `/restore` addresses them.
        picker::Choice::Trash(index) => SlashCommand::Restore(Some((index + 1).to_string())),
        // The archived rows are numbered the way `/sessions unarchive` does.
        picker::Choice::Archived(index) => {
            SlashCommand::Sessions(Some(format!("unarchive {}", index + 1)))
        }
        // A hit's row is numbered from one, and `/find #12` is the line that
        // reveals the row this pick reveals.
        picker::Choice::Find(row) => SlashCommand::Find(format!("#{}", row + 1)),
    }
}

/// The rows a choice list paints: what it chooses, the rows around the
/// highlight, and one row saying what the list holds and how to commit it.
///
/// The detail is right-aligned the way the desktop's picker draws it - the
/// provider beside every model, on the same column for every row - so a reader
/// scans one fact down the list instead of reading it off the end of each row.
fn picker_lines(picker: &picker::Picker, width: usize) -> Vec<String> {
    let rows = picker.rows();
    if rows.is_empty() {
        // A filter that matches nothing leaves the title and the way out: an
        // empty frame would look like the list had closed itself.
        return vec![
            truncate(picker.title(), width),
            truncate(&format!("none match '{}'", picker.query()), width),
            truncate("Backspace erases · Esc closes", width),
        ];
    }
    let query = picker.query();
    let filter = if query.trim().is_empty() {
        String::new()
    } else {
        format!(" · filter '{query}'")
    };
    let mut lines = vec![truncate(
        &format!("{}{filter} · type to narrow", picker.title()),
        width,
    )];
    // The window follows the highlight, keeping it on the last visible row
    // while there are more choices below, the way a scrolling list does.
    let total = picker.len();
    let shown = rows.len();
    // The window is the list's own answer, so the rows this frame draws and
    // the rows a click addresses are the same ones.
    let (start, visible) = picker.window();
    for (offset, item) in rows.iter().enumerate().skip(start).take(visible) {
        // Two columns, so the marks never move the names: the pointer says
        // where the reader is, the tick says what the session already runs,
        // and a row can be both without either mark covering the other.
        let pointer = format!(
            "{}{}",
            if offset == picker.selected() {
                '›'
            } else {
                ' '
            },
            if item.active { '✓' } else { ' ' }
        );
        lines.push(picker_row(&item.label, &item.detail, width, &pointer));
    }
    // What the list holds, and whether more rows follow the window: a reader
    // who filtered a 200-model catalogue down needs to know how many match.
    let count = if shown == total {
        format!("{total} choice(s)")
    } else {
        format!("{shown} of {total} match")
    };
    let more = if start + visible < shown { "… " } else { "" };
    // The tick only explains one of the marks, so it is named only on a list
    // that carries one: the bin and the archive have nothing to mark, and a key
    // to a mark no row shows would be noise on the row that says how to commit.
    let ticked = if picker.any_active() {
        " · ✓ marks the active row"
    } else {
        ""
    };
    lines.push(truncate(
        &format!("{more}{count} · ↑↓ walks · Enter picks · type filters{ticked}"),
        width,
    ));
    lines
}

/// One choice row: the pointer and the name on the left, the fact about it on
/// the right edge.
fn picker_row(label: &str, detail: &str, width: usize, pointer: &str) -> String {
    let head = format!("{pointer} {label}");
    let detail = detail.trim();
    if detail.is_empty() || cells(&head) + cells(detail) + 2 > width {
        return truncate(&head, width);
    }
    // The gap the fit test above guarantees is what the pad fills: padding to
    // the width minus the fact lands the fact on the last column of every row,
    // so the column reads straight down instead of ending one cell short.
    format!("{}{detail}", pad(&head, width - cells(detail)))
}

/// The approval row: the exact action, then the two chords.
///
/// A summary alone is what the desktop shows beside its permission card; the
/// exact action - the tool, what it does and the resource it touches - is what
/// an operator needs before pressing a key that authorizes it.
fn approval_line(approval: &ApprovalView, width: usize) -> String {
    let target = &approval.action;
    let resource = target
        .resource
        .as_deref()
        .filter(|resource| !resource.trim().is_empty())
        .map(|resource| format!(" on {resource}"))
        .unwrap_or_default();
    let summary = approval.summary.trim();
    let detail = if summary.is_empty() {
        String::new()
    } else {
        format!(" — {summary}")
    };
    truncate(
        &format!(
            "approval {}: {}.{}{resource}{detail} · Ctrl-Y approve, Ctrl-R reject",
            short(&approval.approval_id.to_string()),
            target.tool,
            target.action
        ),
        width,
    )
}

/// The card a projected `ask_user` call is still waiting on: its index in the
/// projection and the questions it asked.
///
/// A call is pending when it is the newest `ask_user` call, it completed, its
/// arguments name at least one question, and no reader turn came after it. A
/// user message after the call is the answer, which is exactly how the
/// desktop's card settles: the reader's words arrive as their next message.
fn pending_question(session: &SessionProjection) -> Option<(usize, Vec<AskedQuestion>)> {
    let (index, call) = session
        .tool_calls
        .iter()
        .enumerate()
        .rev()
        .find(|(_, call)| call.tool.trim() == "ask_user")?;
    if call.status != ToolCallStatus::Completed {
        return None;
    }
    if session
        .messages
        .iter()
        .any(|message| message.role == MessageRole::User && message.at >= call.started_at)
    {
        return None;
    }
    let questions = asked_questions(&call.args)?;
    Some((index, questions))
}

/// The questions one `ask_user` call carried, or `None` when its arguments
/// name none.
///
/// The tool refuses a question with fewer than two options, so a malformed item
/// here is dropped rather than drawn as a row a reader could not answer.
fn asked_questions(args: &str) -> Option<Vec<AskedQuestion>> {
    let value: serde_json::Value = serde_json::from_str(args.trim()).ok()?;
    let questions = value.get("questions")?.as_array()?;
    let asked: Vec<AskedQuestion> = questions
        .iter()
        .filter_map(|item| {
            let question = item.get("question")?.as_str()?.trim().to_string();
            if question.is_empty() {
                return None;
            }
            let options: Vec<String> = item
                .get("options")?
                .as_array()?
                .iter()
                .filter_map(|option| option.as_str())
                .map(str::trim)
                .filter(|option| !option.is_empty())
                .map(str::to_string)
                .collect();
            if options.len() < 2 {
                return None;
            }
            Some(AskedQuestion {
                question,
                options,
                multi_select: item
                    .get("multi_select")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect();
    (!asked.is_empty()).then_some(asked)
}

/// The ask_user card's rows: which step, the question, its options and how to
/// answer.
///
/// The desktop draws the same questions as a card inside the transcript with
/// one button per option. A terminal has no buttons, so the card sits where the
/// answer is typed and each option carries the digit that puts the highlight on
/// it.
fn question_lines(card: &QuestionCard, width: usize) -> Vec<String> {
    let Some(question) = card.current() else {
        return Vec::new();
    };
    let total = card.questions.len();
    let pick = if question.multi_select {
        " · multi-select"
    } else {
        ""
    };
    let mut lines = vec![truncate(
        &format!("questions · {}/{total}{pick}", card.step + 1),
        width,
    )];
    // A long question wraps, but it may not take the transcript's rows: three
    // rows carry the gist, and the tool call itself holds the whole text.
    lines.extend(wrap("? ", &question.question, width).into_iter().take(3));
    let picked = card.picks.get(card.step).cloned().unwrap_or_default();
    for (index, option) in question
        .options
        .iter()
        .take(MAX_QUESTION_OPTIONS)
        .enumerate()
    {
        let arrow = if index == card.highlight { "▸" } else { " " };
        let mark = if question.multi_select {
            if picked.iter().any(|pick| pick == option) {
                "[x]"
            } else {
                "[ ]"
            }
        } else {
            "   "
        };
        lines.push(truncate(
            &format!("{arrow}{mark} {} · {option}", index + 1),
            width,
        ));
    }
    lines.push(truncate(
        if question.multi_select {
            "↑↓ walks options · Enter toggles · Ctrl-J moves on · Esc dismisses"
        } else {
            "↑↓ walks options · Enter answers · Esc dismisses · typing answers freely"
        },
        width,
    ));
    lines
}

/// The transcript rows of one projected tool call.
fn tool_lines(call: &ToolCallView, width: usize) -> Vec<String> {
    let (marker, verdict) = match call.status {
        ToolCallStatus::Running => ("⚙ ", "running"),
        ToolCallStatus::Completed => ("✓ ", "done"),
        ToolCallStatus::Failed => ("✗ ", "failed"),
    };
    let summary = if call.summary.is_empty() {
        String::new()
    } else {
        format!(" {}", call.summary)
    };
    let duration = call
        .duration_ms
        .map(|ms| format!(" · {ms}ms"))
        .unwrap_or_default();
    let mut lines = wrap(
        marker,
        &format!("{}{summary}{duration} · {verdict}", call.tool),
        width,
    );
    // A failed call's output is the reader's next question; a completed one's
    // can be long, so only the failure expands.
    if call.status == ToolCallStatus::Failed
        && let Some(output) = &call.output
    {
        for line in output.lines().take(3) {
            lines.extend(wrap("  ", line.trim_end(), width));
        }
    }
    // A landed patch is the change itself. The desktop transcript shows the
    // rows a patch added and removed; a terminal reader gets the same
    // evidence instead of only the one-line summary of the call.
    if call.status == ToolCallStatus::Completed
        && let Some(rows) = patch_rows(&call.args, width)
    {
        lines.extend(rows);
    }
    lines
}

/// The changed rows of a landed `apply_patch` call, if its arguments carry one.
///
/// Only added and removed lines are drawn: a unified diff's file headers are
/// noise in a transcript row, and a long patch is capped so one call cannot
/// take the whole frame. `None` means this call is not a patch, which is the
/// common case and never an error.
fn patch_rows(args: &str, width: usize) -> Option<Vec<String>> {
    let patch = serde_json::from_str::<serde_json::Value>(args)
        .ok()?
        .get("patch")?
        .as_str()?
        .to_string();
    let mut rows: Vec<String> = Vec::new();
    let mut dropped = 0usize;
    for line in patch.lines() {
        let (marker, text) = match line.chars().next() {
            Some('+') if !line.starts_with("+++") => ("+ ", &line[1..]),
            Some('-') if !line.starts_with("---") => ("- ", &line[1..]),
            _ => continue,
        };
        if rows.len() >= PATCH_ROWS {
            dropped += 1;
            continue;
        }
        rows.extend(wrap(&format!("  {marker}"), text, width));
    }
    if rows.is_empty() {
        return None;
    }
    if dropped > 0 {
        rows.push(format!("  … {dropped} more changed row(s)"));
    }
    Some(rows)
}

/// The detail pane: one tool call's title and body, windowed to `rows`.
///
/// The title stays on the first row while the body scrolls, because the title
/// is the pane's own answer to "which call am I looking at?" - a reader who
/// scrolled into the middle of a long output must still be able to tell.
/// The body opens at its first row and `scroll` walks towards its end: a
/// detail is a document the reader asked for, not a log to catch up on, so it
/// reads top-down the way every other pager does.
fn detail_pane(
    session: Option<&SessionProjection>,
    index: usize,
    width: usize,
    rows: usize,
    scroll: usize,
) -> Vec<String> {
    let rows = rows.max(1);
    let Some(call) = session.and_then(|session| session.tool_calls.get(index)) else {
        let mut lines =
            vec!["detail · that tool call is no longer in the projection · Esc closes".to_string()];
        lines.resize(rows, String::new());
        return lines;
    };
    let total = session.map_or(0, |session| session.tool_calls.len());
    let duration = call
        .duration_ms
        .map(|ms| format!(" · {ms}ms"))
        .unwrap_or_default();
    let title = format!(
        "detail {}/{} · {} · {}{} · Up/Down scrolls · Esc closes",
        index + 1,
        total,
        call.tool,
        status_word(call.status),
        duration
    );
    let body_rows = rows.saturating_sub(1);
    let body = fold_rows(detail_block(call, width), DETAIL_ROWS);
    let body_total = body.len();
    // Only content taller than the pane can scroll, so a short call never
    // scrolls away from its first row however many times the key was pressed.
    let start = scroll.min(body_total.saturating_sub(body_rows));
    let end = (start + body_rows).min(body_total);
    let mut lines = Vec::with_capacity(rows);
    lines.push(truncate(&title, width));
    lines.extend(body[start..end].iter().cloned());
    // The pane always fills the rows it was given, so the composer below it
    // never jumps when a call's body is short.
    lines.resize(rows, String::new());
    lines
}

/// The body of one tool call's detail: its arguments, summary and output.
///
/// The transcript already draws the call's name, verdict and changed rows; this
/// is the rest of what the Host recorded - the request the model made and the
/// whole output it got back - because a one-line summary cannot answer "what
/// exactly did it run?".
fn detail_block(call: &ToolCallView, width: usize) -> Vec<String> {
    let mut lines = vec!["args".to_string()];
    lines.extend(indented("  ", call.args.trim(), width));
    if !call.summary.trim().is_empty() {
        lines.push("summary".to_string());
        lines.extend(indented("  ", call.summary.trim(), width));
    }
    match call.output.as_deref().map(str::trim_end) {
        Some(output) if !output.trim().is_empty() => {
            lines.push("output".to_string());
            lines.extend(indented("  ", output, width));
        }
        // A call still running has no output yet; one that finished with an
        // empty result has an empty one. The verdict already says which, so
        // this row only records that there is nothing to read.
        _ => lines.push(format!("output · none · {}", status_word(call.status))),
    }
    lines
}

/// One text block with a two-cell gutter, breaking wide rows at the pane.
///
/// Unlike the transcript's [`wrap`], this keeps a source line's own leading
/// space: arguments are JSON and output is often a patch or a table, and
/// collapsing their indentation would change what the reader is reading. A row
/// narrower than the pane is kept whole; a wider one is split at the edge.
fn indented(prefix: &str, text: &str, width: usize) -> Vec<String> {
    let gutter = cells(prefix);
    let room = width.saturating_sub(gutter).max(8);
    let indent = " ".repeat(gutter);
    let mut lines: Vec<String> = Vec::new();
    for source in text.split('\n') {
        if source.is_empty() {
            lines.push(String::new());
            continue;
        }
        for (index, chunk) in break_cells(source, room).into_iter().enumerate() {
            lines.push(if index == 0 {
                format!("{prefix}{chunk}")
            } else {
                format!("{indent}{chunk}")
            });
        }
    }
    lines
}

/// Fold a detail body past `cap` rows into one counted row.
///
/// The counted row is itself a row, so the result never exceeds `cap`.
fn fold_rows(mut lines: Vec<String>, cap: usize) -> Vec<String> {
    if lines.len() <= cap {
        return lines;
    }
    let kept = cap.saturating_sub(1);
    let dropped = lines.len() - kept;
    lines.truncate(kept);
    lines.push(format!("… {dropped} more row(s)"));
    lines
}

/// The word a call's state is drawn with, shared by the transcript and the
/// detail title so one call never reads two different ways.
fn status_word(status: ToolCallStatus) -> &'static str {
    match status {
        ToolCallStatus::Running => "running",
        ToolCallStatus::Completed => "done",
        ToolCallStatus::Failed => "failed",
    }
}

/// A 0xRRGGBB colour as a ratatui colour (the shell grid's cells).
fn rgb(color: u32) -> Color {
    Color::Rgb(
        ((color >> 16) & 0xFF) as u8,
        ((color >> 8) & 0xFF) as u8,
        (color & 0xFF) as u8,
    )
}

/// One horizontal rule, exactly `width` cells.
fn rule(width: usize) -> String {
    "─".repeat(width)
}

/// The width of `text` in terminal cells.
///
/// A row is measured in the cells the terminal paints, not in characters: a
/// CJK ideograph or an emoji takes two columns and a combining mark none, so
/// counting characters lands a Chinese title one column past the frame's right
/// border and wraps a paragraph a cell too wide. Every writer in this module
/// measures with this, so a title, a wrapped paragraph, the composer's cursor
/// and the frame's borders all agree on what fits.
fn cells(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// Break `text` into pieces of at most `room` cells each, never splitting a
/// character: a wide glyph that would straddle the edge starts the next piece
/// instead. This is the one splitter the panes share, so a document, a tool's
/// output and a code block all break in the same column.
fn break_cells(text: &str, room: usize) -> Vec<String> {
    let room = room.max(1);
    let mut pieces: Vec<String> = Vec::new();
    let mut piece = String::new();
    let mut used = 0usize;
    for character in text.chars() {
        let advance = UnicodeWidthChar::width(character).unwrap_or(0);
        if used > 0 && used + advance > room {
            pieces.push(std::mem::take(&mut piece));
            used = 0;
        }
        piece.push(character);
        used += advance;
    }
    pieces.push(piece);
    pieces
}

fn truncate(text: &str, width: usize) -> String {
    if cells(text) <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    // The ellipsis is one cell, so the text keeps the other `width - 1`.
    let room = width - 1;
    let mut kept = String::new();
    let mut used = 0;
    for character in text.chars() {
        let advance = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + advance > room {
            break;
        }
        kept.push(character);
        used += advance;
    }
    kept.push('…');
    kept
}

fn pad(text: &str, width: usize) -> String {
    let mut padded = truncate(text, width);
    for _ in cells(&padded)..width {
        padded.push(' ');
    }
    padded
}

/// A short display form of a uuid: enough to tell two approvals apart.
fn short(value: &str) -> String {
    value.chars().take(8).collect()
}

/// Wrap `text` to `width` cells under `prefix`, breaking at spaces first and
/// mid-word only when a single word cannot fit.
fn wrap(prefix: &str, text: &str, width: usize) -> Vec<String> {
    let indent = " ".repeat(cells(prefix));
    wrap_rows(prefix, &indent, text, width)
}

/// [`wrap`], with a different prefix for every row after the first.
///
/// A list item's words hang under the item's first word rather than under its
/// glyph: the reader scans the items' left edge, not the bullets.
fn wrap_hanging(prefix: &str, hang: &str, text: &str, width: usize) -> Vec<String> {
    wrap_rows(prefix, hang, text, width)
}

/// The wrapper both forms share: `prefix` opens the first row, `hang` opens
/// every row after it, and the room is what the wider of the two leaves.
fn wrap_rows(prefix: &str, hang: &str, text: &str, width: usize) -> Vec<String> {
    let margin = cells(prefix).max(cells(hang));
    let room = width.max(margin + 8) - margin;
    let indent = hang.to_string();
    let mut lines: Vec<String> = Vec::new();
    for paragraph in text.split('\n') {
        let mut current = String::new();
        let mut used = 0usize;
        let mut started = false;
        for word in paragraph.split_whitespace() {
            let word_cells = cells(word);
            if started && used + 1 + word_cells <= room {
                current.push(' ');
                current.push_str(word);
                used += 1 + word_cells;
                continue;
            }
            if started {
                lines.push(std::mem::take(&mut current));
                used = 0;
            }
            // A word longer than the row is broken instead of lost, at the
            // last cell boundary that still fits a whole character.
            for character in word.chars() {
                let advance = UnicodeWidthChar::width(character).unwrap_or(0);
                if used > 0 && used + advance > room {
                    lines.push(std::mem::take(&mut current));
                    used = 0;
                }
                current.push(character);
                used += advance;
            }
            started = true;
        }
        lines.push(current);
    }
    lines
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                format!("{prefix}{line}")
            } else {
                format!("{indent}{line}")
            }
        })
        .collect()
}

/// The row that opens a fenced block, naming its language when it has one.
const FENCE_OPEN: &str = "╭";
/// The row that closes a fenced block.
const FENCE_CLOSE: &str = "╰";
/// The gutter every block row carries, corners included. A row that is only
/// this is a blank line inside the block, which is still inside it.
const FENCE_GUTTER: &str = "│";

/// One message's rows: its prose wrapped under the speaker's prefix, and every
/// fenced block drawn as a box that keeps the lines it was written in.
///
/// The desktop renders an answer as markdown, so a fenced block there is a
/// block. A terminal that reflowed it as prose would collapse the indentation
/// that makes code readable, so inside a fence the text is handed to the frame
/// as it was written - a tab becomes four spaces, and only a line wider than
/// the frame is broken, under the same gutter, so a reader can still copy a
/// line off the screen and run it. Outside a fence the same answer is drawn
/// through [`prose`], so a heading and a list arrive as a heading and a list
/// instead of as the marks that wrote them.
fn message_rows(speaker: &str, text: &str, width: usize) -> Vec<String> {
    let indent = " ".repeat(cells(speaker));
    let mut rows: Vec<String> = Vec::new();
    // Whether the speaker has been named yet: the first row of a message says
    // who is talking, and every row after it hangs under that name.
    let mut named = false;
    let mut in_fence = false;
    // A message's own trailing newline draws nothing.
    let text = text.trim_end_matches('\n');
    if text.is_empty() {
        return rows;
    }
    for line in text.split('\n') {
        if in_fence {
            match fence_open(line) {
                Some(_) => {
                    close_fence(&mut rows);
                    in_fence = false;
                }
                None => rows.extend(code_rows(line, width)),
            }
            continue;
        }
        let Some(tag) = fence_open(line) else {
            push_line(&mut rows, line, speaker, &indent, &mut named, width);
            continue;
        };
        // A message that opens on a block still names its speaker, or the
        // reader cannot tell an answer from the question above it.
        if !named {
            rows.push(speaker.to_string());
            named = true;
        }
        rows.push(fence_label(tag, width));
        in_fence = true;
    }
    if in_fence {
        close_fence(&mut rows);
    }
    rows
}

/// Whether a line opens or closes a fence, and the language it names.
///
/// A fence is three backticks or three tildes at the start of a line, which is
/// the whole rule markdown uses; the info string's first word is the language,
/// and any further words are options this client does not draw.
fn fence_open(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let rest = trimmed
        .strip_prefix("```")
        .or_else(|| trimmed.strip_prefix("~~~"))?;
    Some(rest.split_whitespace().next().unwrap_or(""))
}

/// The block's opening row: a corner, and the language when the fence named
/// one.
fn fence_label(tag: &str, width: usize) -> String {
    let room = width.saturating_sub(2);
    let tag = truncate(tag, room);
    if tag.is_empty() {
        FENCE_OPEN.to_string()
    } else {
        format!("{FENCE_OPEN} {tag}")
    }
}

/// Close a block, dropping the blank rows its text ended on: a fence at the end
/// of a message must not draw an empty gutter row above its own corner.
fn close_fence(rows: &mut Vec<String>) {
    while rows.last().is_some_and(|row| row.as_str() == FENCE_GUTTER) {
        rows.pop();
    }
    rows.push(FENCE_CLOSE.to_string());
}

/// One code line's rows: the gutter, then the line as written.
fn code_rows(line: &str, width: usize) -> Vec<String> {
    let line = line.replace('\t', "    ");
    let line = line.trim_end();
    if line.is_empty() {
        return vec![FENCE_GUTTER.to_string()];
    }
    // Two cells belong to the gutter, so the code gets the rest. A code line
    // is broken at a cell boundary, not a character count: a Chinese comment
    // in a block must not push the gutter out of its column.
    let room = width.saturating_sub(2).max(8);
    break_cells(line, room)
        .into_iter()
        .map(|chunk| format!("{FENCE_GUTTER} {chunk}"))
        .collect()
}

/// The bar a heading opens with.
///
/// The desktop makes a heading bold and larger; a terminal's weights are the
/// terminal's own, so a heading is marked by the bar beside it, inside the
/// message's own indent.
const HEADING_MARK: &str = "▌";
/// The bar a quoted line opens with, one per `>` it was written under.
const QUOTE_MARK: &str = "▏";
/// The glyph a list item opens with, one per nesting depth: markdown nests by
/// indentation, and a model's outline flattened into one glyph loses its shape.
const BULLET_MARKS: [&str; 3] = ["•", "◦", "▪"];
/// The box a task item opens with: markdown's `- [ ]` and `- [x]`.
const TASK_OPEN: &str = "☐";
const TASK_DONE: &str = "☑";

/// What one line of a message's prose is, as markdown writes it.
///
/// The desktop hands an answer to a markdown renderer, so a heading there is a
/// heading and a list is a list. A terminal draws those shapes with the glyphs
/// the rest of this client uses, and anything unrecognised stays the text it
/// was.
#[derive(Debug, PartialEq, Eq)]
enum Prose<'a> {
    /// A blank line: the reader's paragraph break.
    Blank,
    /// Words to wrap as they are.
    Text(&'a str),
    /// A heading's words; its level is the `#`s, which one bar does not draw.
    Heading(&'a str),
    /// A list item: where it nests, and what it says.
    Item { depth: usize, text: &'a str },
    /// A checkbox item, and whether it is ticked.
    Task { done: bool, text: &'a str },
    /// An ordered item: the number as written, and what it says.
    Numbered { number: &'a str, text: &'a str },
    /// A quoted line: how many `>` it opened with, and what it quotes.
    Quote { depth: usize, text: &'a str },
    /// `---`, `***` or `___`: the answer's own horizontal rule.
    Rule,
}

/// What one source line of prose is.
fn prose(line: &str) -> Prose<'_> {
    let line = line.trim_end();
    if line.trim().is_empty() {
        return Prose::Blank;
    }
    let body = line.trim_start();
    // Nesting is what the leading spaces say: two cells is one level, and an
    // outline deeper than the glyphs this client has folds into the last one.
    let depth = ((line.len() - body.len()) / 2).min(BULLET_MARKS.len() - 1);
    if let Some(text) = heading(body) {
        return Prose::Heading(text);
    }
    if is_rule(body) {
        return Prose::Rule;
    }
    if let Some(quoted) = body.strip_prefix('>') {
        let marks = body
            .chars()
            .take_while(|character| *character == '>')
            .count();
        return Prose::Quote {
            depth: marks.clamp(1, 3),
            text: quoted.trim_start_matches('>').trim_start(),
        };
    }
    let marker = body.chars().next().unwrap_or(' ');
    // A list item's marker is `-`, `*` or `+` followed by a space; the first
    // character of a word is not a marker.
    if matches!(marker, '-' | '*' | '+') && body[1..].starts_with(' ') {
        let rest = body[1..].trim_start();
        for (token, done) in [("[ ]", false), ("[x]", true), ("[X]", true)] {
            if let Some(text) = rest.strip_prefix(token) {
                return Prose::Task {
                    done,
                    text: text.trim_start(),
                };
            }
        }
        return Prose::Item { depth, text: rest };
    }
    // An ordered item keeps the number it was written with: renumbering a
    // model's list would argue with the text that refers to its steps.
    if marker.is_ascii_digit() {
        let digits = body
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .count();
        let after = &body[digits..];
        if let Some(text) = after
            .strip_prefix(". ")
            .or_else(|| after.strip_prefix(") "))
        {
            return Prose::Numbered {
                number: &body[..digits],
                text: text.trim_start(),
            };
        }
    }
    Prose::Text(body)
}

/// A heading's words, when the line opens with one to six `#`s and a space.
///
/// `#tag` is a word, not a heading: markdown wants the space, so a line that
/// skips it stays the text it was.
fn heading(body: &str) -> Option<&str> {
    let hashes = body
        .chars()
        .take_while(|character| *character == '#')
        .count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &body[hashes..];
    if rest.is_empty() {
        return Some(rest);
    }
    rest.starts_with(' ').then(|| rest.trim_start())
}

/// Whether the line is a horizontal rule: three or more of `-`, `*` or `_`,
/// which is the whole rule markdown uses.
fn is_rule(body: &str) -> bool {
    let mut marks = body.chars().filter(|character| !character.is_whitespace());
    let Some(first) = marks.next() else {
        return false;
    };
    if !matches!(first, '-' | '*' | '_') {
        return false;
    }
    let mut count = 1;
    for character in marks {
        if character != first {
            return false;
        }
        count += 1;
    }
    count >= 3
}

/// One prose source line, drawn the way the desktop's markdown renders it.
fn push_line(
    rows: &mut Vec<String>,
    line: &str,
    speaker: &str,
    indent: &str,
    named: &mut bool,
    width: usize,
) {
    // The first row of a message names its speaker; the rows after it hang
    // under that name.
    let first = if *named { indent } else { speaker };
    match prose(line) {
        Prose::Blank => {
            // Nothing drawn yet means there is no paragraph to break: a message
            // that opens on a blank line starts on its first word.
            if rows.is_empty() {
                return;
            }
            rows.push(first.to_string());
        }
        Prose::Text(text) => rows.extend(wrap(first, text, width)),
        Prose::Heading(text) => block(
            rows,
            first,
            indent,
            &format!("{HEADING_MARK} "),
            text,
            width,
        ),
        Prose::Item { depth, text } => {
            let glyph = format!("{} ", BULLET_MARKS[depth]);
            block(rows, first, indent, &glyph, text, width);
        }
        Prose::Task { done, text } => {
            let glyph = if done { TASK_DONE } else { TASK_OPEN };
            block(rows, first, indent, &format!("{glyph} "), text, width);
        }
        Prose::Numbered { number, text } => {
            block(rows, first, indent, &format!("{number}. "), text, width);
        }
        Prose::Quote { depth, text } => {
            let bar = format!("{} ", QUOTE_MARK.repeat(depth));
            block(rows, first, indent, &bar, text, width);
        }
        // A rule is a row of its own at the frame's full width, without the
        // speaker's indent: it separates the message from itself, and the
        // frame paints a row of `─` as the rule it is.
        Prose::Rule => rows.push("─".repeat(width)),
    }
    *named = true;
}

/// Wrap a marked block: its glyph joins the speaker on the message's first
/// row, and every row after it hangs under the block's words.
fn block(rows: &mut Vec<String>, first: &str, indent: &str, glyph: &str, text: &str, width: usize) {
    let head = format!("{first}{glyph}");
    let hang = format!("{indent}{}", " ".repeat(cells(glyph)));
    rows.extend(wrap_hanging(&head, &hang, text, width));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::contract::{
        ApprovalTarget, ReviewDecisionView, ReviewDiffLineView, ReviewFindingView, ReviewNodeView,
        ReviewVerificationView, TaskSectionView,
    };

    #[test]
    fn wrapping_keeps_text_and_breaks_long_words() {
        let wrapped = wrap("you › ", "hello world this is a longer sentence", 20);
        assert_eq!(wrapped[0], "you › hello world");
        assert!(wrapped.iter().all(|line| line.chars().count() <= 20));

        let long = wrap("› ", &"x".repeat(40), 10);
        assert!(long.iter().all(|line| line.chars().count() <= 10));
        // The marker rides on the first row, so count the payload rather than
        // the line: trimming keeps the marker and could never equal 40 x's.
        assert_eq!(
            long.iter()
                .map(|line| line.matches('x').count())
                .sum::<usize>(),
            40
        );
    }

    #[test]
    fn a_fenced_block_is_a_box_that_keeps_its_indentation() {
        let rows = message_rows(
            "bsh › ",
            "here is the fix:\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\nthat's all",
            40,
        );
        assert_eq!(rows[0], "bsh › here is the fix:");
        assert_eq!(rows[1], "╭ rust", "the fence's language names the block");
        assert_eq!(rows[2], "│ fn main() {");
        assert_eq!(
            rows[3], "│     println!(\"hi\");",
            "code is not reflowed: its indentation is the code"
        );
        assert_eq!(rows[4], "│ }");
        assert_eq!(rows[5], "╰");
        assert_eq!(
            rows[6].trim(),
            "that's all",
            "prose after a block hangs under the speaker it already named"
        );
        assert!(
            rows[6].starts_with("      "),
            "the speaker said who is talking; the rest of the message is indented under it"
        );
    }

    #[test]
    fn a_code_line_wider_than_the_frame_breaks_under_its_gutter() {
        let rows = message_rows(">> ", "```\nabcdefghij\n\nk\n```", 10);
        assert_eq!(rows[0], ">> ", "the block names its speaker");
        assert_eq!(rows[1], "╭", "an untagged fence opens on the corner alone");
        assert_eq!(rows[2], "│ abcdefgh");
        assert_eq!(rows[3], "│ ij");
        assert_eq!(
            rows[4], "│",
            "a blank line inside a block is still inside it"
        );
        assert_eq!(rows[5], "│ k");
        assert_eq!(rows[6], "╰");
        assert!(rows.iter().all(|row| row.chars().count() <= 10));
    }

    #[test]
    fn a_block_that_never_closes_is_closed_and_the_speaker_still_speaks() {
        let rows = message_rows("you › ", "```\nls -la\n", 30);
        assert_eq!(rows[0], "you › ", "the block names its speaker");
        assert_eq!(rows[1], "╭");
        assert_eq!(rows[2], "│ ls -la");
        assert_eq!(rows[3], "╰");
        assert_eq!(
            rows.len(),
            4,
            "the message's own trailing newline draws nothing"
        );
    }

    #[test]
    fn a_message_without_a_fence_wraps_the_way_it_always_did() {
        let text = "hello world this is a longer sentence";
        assert_eq!(message_rows("you › ", text, 20), wrap("you › ", text, 20));
    }

    #[test]
    fn prose_is_read_the_way_markdown_writes_it() {
        assert_eq!(prose("plain words"), Prose::Text("plain words"));
        assert_eq!(prose("## Steps"), Prose::Heading("Steps"));
        assert_eq!(
            prose("#tag is not a heading"),
            Prose::Text("#tag is not a heading"),
            "markdown wants the space, so a word that starts with # stays a word"
        );
        assert_eq!(
            prose("- one"),
            Prose::Item {
                depth: 0,
                text: "one"
            }
        );
        assert_eq!(
            prose("    - deep"),
            Prose::Item {
                depth: 2,
                text: "deep"
            },
            "four spaces is two levels of nesting"
        );
        assert_eq!(
            prose("- [x] done"),
            Prose::Task {
                done: true,
                text: "done"
            }
        );
        assert_eq!(
            prose("- [ ] open"),
            Prose::Task {
                done: false,
                text: "open"
            }
        );
        assert_eq!(
            prose("2) second"),
            Prose::Numbered {
                number: "2",
                text: "second"
            }
        );
        assert_eq!(
            prose(">> quoted"),
            Prose::Quote {
                depth: 2,
                text: "quoted"
            }
        );
        assert_eq!(prose("---"), Prose::Rule);
        assert_eq!(prose("  "), Prose::Blank);
        assert_eq!(
            prose("-5 degrees"),
            Prose::Text("-5 degrees"),
            "a minus sign without a space after it is arithmetic, not a list"
        );
    }

    #[test]
    fn a_markdown_answer_keeps_its_headings_lists_and_rules() {
        let rows = message_rows(
            "bsh › ",
            "## Plan\n- first step\n- [x] done\n> note\n---\ntext",
            40,
        );
        assert_eq!(rows[0], "bsh › ▌ Plan", "a heading is marked, not spelled");
        assert_eq!(
            rows[1], "      • first step",
            "an item hangs under the speaker"
        );
        assert_eq!(
            rows[2], "      ☑ done",
            "a ticked box is the glyph of the task"
        );
        assert_eq!(rows[3], "      ▏ note");
        assert_eq!(
            rows[4],
            "─".repeat(40),
            "a rule is a row of its own, at the frame's full width"
        );
        assert_eq!(rows[5].trim(), "text");
        assert!(
            rows.iter()
                .all(|row| !row.contains("##") && !row.contains("- ")),
            "no markdown marker reaches the screen: {rows:?}"
        );
    }

    #[test]
    fn a_long_list_item_hangs_under_its_words_not_under_its_bullet() {
        let rows = message_rows("you › ", "- alpha beta gamma delta", 20);
        assert_eq!(
            rows[0], "you › • alpha beta",
            "the speaker and the glyph share the first row"
        );
        assert!(rows.len() > 1, "the item wraps: {rows:?}");
        assert!(
            rows[1].starts_with("        "),
            "the text hangs two cells past the speaker, under the first word: {rows:?}"
        );
        assert!(
            rows.iter().all(|row| cells(row) <= 20),
            "the hanging indent still fits the frame: {rows:?}"
        );
    }

    #[test]
    fn wide_text_fills_the_columns_it_occupies_not_the_characters_it_has() {
        // Ten ideographs are twenty cells. Counting characters would put them
        // on one row of "ten" and paint it past the frame's right border, which
        // is what a Chinese paragraph looked like before this measured cells.
        let text = "日本語の説明文です。";
        let rows = wrap("› ", text, 12);
        assert!(
            rows.iter().all(|row| cells(row) <= 12),
            "every row fits the frame: {rows:?}"
        );
        assert!(
            rows.len() > 1,
            "one row's worth of characters is more than the frame holds"
        );
        assert!(
            rows[1].starts_with("  "),
            "continuation hangs under the speaker: {rows:?}"
        );
    }

    #[test]
    fn a_wide_code_line_breaks_at_a_whole_character() {
        let rows = message_rows(">> ", "```\n日本語のコメント\n```", 10);
        assert!(
            rows.iter().all(|row| cells(row) <= 10),
            "the gutter stays in its column: {rows:?}"
        );
        assert_eq!(rows[2], "│ 日本語の", "four wide glyphs are eight cells");
        assert!(
            rows.iter().any(|row| row == "│ コメント"),
            "the line continues under the same gutter: {rows:?}"
        );
    }

    #[test]
    fn truncation_keeps_the_cells_it_promised() {
        assert_eq!(cells("日本語"), 6);
        assert_eq!(truncate("日本語テキスト", 8), "日本語…");
        assert_eq!(cells(&truncate("日本語テキスト", 8)), 7);
        assert_eq!(cells(&pad("日本語", 8)), 8);
    }

    #[test]
    fn slash_lines_parse_and_mistakes_stay_local() {
        assert_eq!(SlashCommand::parse("/help"), Some(SlashCommand::Help));
        assert_eq!(SlashCommand::parse("/compact"), Some(SlashCommand::Compact));
        assert!(matches!(
            SlashCommand::parse("/compact now"),
            Some(SlashCommand::Usage(_))
        ));
        assert_eq!(
            SlashCommand::parse("/rename"),
            Some(SlashCommand::Rename(None))
        );
        assert_eq!(
            SlashCommand::parse("/rename Release the hounds"),
            Some(SlashCommand::Rename(Some("Release the hounds".to_string()))),
            "a title keeps the spaces between its words"
        );
        assert_eq!(SlashCommand::parse("/plan"), Some(SlashCommand::Plan(None)));
        assert_eq!(
            SlashCommand::parse("/plan off"),
            Some(SlashCommand::Plan(Some(false)))
        );
        assert_eq!(
            SlashCommand::parse("/model test fast"),
            Some(SlashCommand::Model(
                Some("test".to_string()),
                Some("fast".to_string())
            ))
        );
        assert!(matches!(
            SlashCommand::parse("/model test"),
            Some(SlashCommand::Usage(_))
        ));
        assert!(matches!(
            SlashCommand::parse("/plan maybe"),
            Some(SlashCommand::Usage(_))
        ));
        assert_eq!(
            SlashCommand::parse("/queue clear"),
            Some(SlashCommand::Queue(Some("clear".to_string())))
        );
        assert_eq!(
            SlashCommand::parse("/queue"),
            Some(SlashCommand::Queue(None))
        );
        // The verdicts: approving takes nothing, and refusing keeps the reader's
        // reason whole, because that sentence is the note the audit records.
        assert_eq!(SlashCommand::parse("/approve"), Some(SlashCommand::Approve));
        assert!(matches!(
            SlashCommand::parse("/approve now"),
            Some(SlashCommand::Usage(_))
        ));
        assert_eq!(SlashCommand::parse("/deny"), Some(SlashCommand::Deny(None)));
        assert_eq!(
            SlashCommand::parse("/deny the target is production"),
            Some(SlashCommand::Deny(Some(
                "the target is production".to_string()
            )))
        );
        // `/sessions` takes a number or free-text filter: a title carries
        // spaces, so the whole argument is kept.
        assert_eq!(
            SlashCommand::parse("/sessions"),
            Some(SlashCommand::Sessions(None))
        );
        assert_eq!(
            SlashCommand::parse("/sessions 2"),
            Some(SlashCommand::Sessions(Some("2".to_string())))
        );
        assert_eq!(
            SlashCommand::parse("/sessions release the hounds"),
            Some(SlashCommand::Sessions(Some(
                "release the hounds".to_string()
            )))
        );
        // `/copy` reads the newest answer, a count back from it, or the
        // desktop session menu's other row: the open session's id.
        assert_eq!(SlashCommand::parse("/copy"), Some(SlashCommand::Copy(None)));
        assert_eq!(
            SlashCommand::parse("/copy 2"),
            Some(SlashCommand::Copy(Some(2)))
        );
        assert_eq!(SlashCommand::parse("/copy id"), Some(SlashCommand::CopyId));
        assert_eq!(
            SlashCommand::parse("/copy ID"),
            Some(SlashCommand::CopyId),
            "the word is a noun, not a case-sensitive key"
        );
        // Zero names a message before the newest one, and a second word is a
        // sentence this command does not read.
        assert!(matches!(
            SlashCommand::parse("/copy 0"),
            Some(SlashCommand::Usage(_))
        ));
        assert!(matches!(
            SlashCommand::parse("/copy id now"),
            Some(SlashCommand::Usage(_))
        ));
        // A branch names nothing: the Host derives the chat from the open
        // session, so an argument is a usage line rather than a title.
        assert_eq!(SlashCommand::parse("/fork"), Some(SlashCommand::Fork));
        assert!(matches!(
            SlashCommand::parse("/fork release"),
            Some(SlashCommand::Usage(_))
        ));

        assert_eq!(
            SlashCommand::parse("/feedback"),
            Some(SlashCommand::Feedback(None))
        );
        assert_eq!(
            SlashCommand::parse("/feedback clear"),
            Some(SlashCommand::Feedback(Some("clear".to_string())))
        );
        assert_eq!(
            SlashCommand::parse("/feedback the diff is too noisy"),
            Some(SlashCommand::Feedback(Some(
                "the diff is too noisy".to_string()
            )))
        );
        // The control word fills the whole argument; anything longer is a note.
        assert!(!clears_feedback("clear the queue"));
        assert!(clears_feedback("CLEAR"), "the control word ignores case");
        assert_eq!(
            SlashCommand::parse("/nonsense"),
            Some(SlashCommand::Unknown("/nonsense".to_string()))
        );
        // A line with no slash is a turn, even when it mentions a command.
        assert_eq!(SlashCommand::parse("help me plan this"), None);
    }

    #[test]
    fn goal_lines_follow_the_desktop_grammar() {
        assert_eq!(SlashCommand::parse("/goal"), Some(SlashCommand::GoalShow));
        // The control words fill the whole argument; case does not matter, the
        // way `parse_goal_request` lowercases before comparing.
        assert_eq!(
            SlashCommand::parse("/goal CLEAR"),
            Some(SlashCommand::GoalClear)
        );
        assert_eq!(
            SlashCommand::parse("/goal Pause"),
            Some(SlashCommand::GoalPause)
        );
        assert_eq!(
            SlashCommand::parse("/goal resume"),
            Some(SlashCommand::GoalResume)
        );
        // A control word with words after it is an objective, not a refusal.
        assert_eq!(
            SlashCommand::parse("/goal pause after verification"),
            Some(SlashCommand::GoalSet(
                "pause after verification".to_string()
            ))
        );
        assert_eq!(
            SlashCommand::parse("/goal edit Ship the release"),
            Some(SlashCommand::GoalEdit("Ship the release".to_string()))
        );
        assert!(matches!(
            SlashCommand::parse("/goal edit"),
            Some(SlashCommand::Usage(_))
        ));
        assert_eq!(
            SlashCommand::parse("/goal Ship   the release"),
            Some(SlashCommand::GoalSet("Ship   the release".to_string()))
        );
    }

    #[test]
    fn an_approval_row_names_the_exact_action() {
        let approval = ApprovalView {
            approval_id: crate::runtime::ids::ApprovalId::new(),
            session_id: SessionId::new(),
            execution_id: ExecutionId::new(),
            summary: "remove the build directory".to_string(),
            action: ApprovalTarget {
                tool: "shell".to_string(),
                action: "run".to_string(),
                resource: Some("project/build".to_string()),
            },
            action_digest: crate::runtime::ids::ActionDigest::of(&"shell.run"),
            status: ApprovalStatus::Pending,
            expires_at: Utc::now(),
        };
        let line = approval_line(&approval, 200);
        assert!(
            line.contains("shell.run on project/build"),
            "the tool, its action and the resource are named: {line}"
        );
        assert!(
            line.contains("— remove the build directory"),
            "the operator-facing summary rides along: {line}"
        );
        // The row still narrows to the frame it is painted into.
        assert!(approval_line(&approval, 40).chars().count() <= 40);
        // A target with no resource and no summary is a clean two-part row.
        let bare = ApprovalView {
            summary: String::new(),
            action: ApprovalTarget {
                tool: "turn".to_string(),
                action: "execute".to_string(),
                resource: None,
            },
            ..approval
        };
        let line = approval_line(&bare, 200);
        assert!(line.contains("turn.execute · Ctrl-Y"), "{line}");
    }

    #[test]
    fn a_tool_row_names_the_call_and_its_verdict() {
        let at = Utc::now();
        let mut call = ToolCallView {
            execution_id: None,
            step: 1,
            tool: "shell".to_string(),
            summary: "ls -la".to_string(),
            args: "{}".to_string(),
            status: ToolCallStatus::Running,
            output: None,
            duration_ms: None,
            started_at: at,
            finished_at: None,
        };
        let running = tool_lines(&call, 60);
        assert_eq!(running[0], "⚙ shell ls -la · running");

        call.status = ToolCallStatus::Completed;
        call.duration_ms = Some(12);
        assert_eq!(tool_lines(&call, 60)[0], "✓ shell ls -la · 12ms · done");

        call.status = ToolCallStatus::Failed;
        call.output = Some("exit 1\nno such file\nmore detail".to_string());
        let failed = tool_lines(&call, 60);
        assert_eq!(failed[0], "✗ shell ls -la · 12ms · failed");
        assert_eq!(failed[1], "  exit 1");
        assert_eq!(failed.len(), 4, "three output rows, never the whole log");
    }

    #[test]
    fn a_landed_patch_shows_the_rows_it_changed() {
        let at = Utc::now();
        let patch = "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-let old = 1;\n+let new = 2;\n*** End Patch";
        let call = ToolCallView {
            execution_id: None,
            step: 2,
            tool: "apply_patch".to_string(),
            summary: "src/lib.rs".to_string(),
            args: serde_json::json!({ "patch": patch }).to_string(),
            status: ToolCallStatus::Completed,
            output: None,
            duration_ms: Some(4),
            started_at: at,
            finished_at: Some(at),
        };
        let lines = tool_lines(&call, 60);
        assert_eq!(lines[0], "✓ apply_patch src/lib.rs · 4ms · done");
        assert_eq!(lines[1], "  - let old = 1;");
        assert_eq!(lines[2], "  + let new = 2;");

        // A call that is not a patch adds no rows, and neither does a failed
        // one: its output is what the reader needs there.
        let mut plain = call.clone();
        plain.args = "{}".to_string();
        assert_eq!(tool_lines(&plain, 60).len(), 1);
        let mut failed = call.clone();
        failed.status = ToolCallStatus::Failed;
        assert_eq!(tool_lines(&failed, 60).len(), 1);
    }

    #[test]
    fn a_config_summary_names_every_chip() {
        let mut config = SessionConfig::default();
        assert_eq!(
            config_summary(&config),
            "default model · default preset · workspace-write"
        );
        config.provider = Some("test".to_string());
        config.model = Some("fast".to_string());
        config.preset = Some("careful".to_string());
        config.permission = "read-only".to_string();
        config.plan_mode = true;
        assert_eq!(
            config_summary(&config),
            "test/fast · careful · read-only · plan on"
        );
    }

    #[test]
    fn prompt_history_walks_back_and_returns_to_the_draft() {
        let mut history = PromptHistory::default();
        assert_eq!(history.older("typing"), None, "nothing submitted yet");
        history.record("first");
        history.record("second");
        history.record("second");
        assert_eq!(history.older("typing"), Some("second".to_string()));
        assert_eq!(history.older("typing"), Some("first".to_string()));
        assert_eq!(history.older("typing"), None, "the oldest entry is the end");
        assert_eq!(history.newer(), Some("second".to_string()));
        assert_eq!(
            history.newer(),
            Some("typing".to_string()),
            "the draft comes back"
        );
        assert_eq!(history.newer(), None, "past the draft is not a binding");
        // Submitting clears the browsing position, so the next Up is the last
        // line again rather than a stale walk.
        history.record("third");
        assert_eq!(history.older(""), Some("third".to_string()));
    }

    #[test]
    fn the_composer_wraps_and_keeps_the_cursor_on_its_row() {
        // One long line wraps into as many rows as the window needs, and the
        // cursor lands on the row holding the character it precedes.
        let (rows, cursor) = composer(&"x".repeat(30), 20, 12);
        assert_eq!(rows.len(), 3, "ten cells per row");
        assert_eq!(rows[0], "› xxxxxxxxxx");
        assert_eq!(rows[1], "  xxxxxxxxxx");
        // Twenty characters fill two rows exactly, so the cursor waits at the
        // end of the second one; the character it precedes opens the third.
        assert_eq!(cursor, (1, 12));
        let (_, cursor) = composer(&"x".repeat(30), 21, 12);
        assert_eq!(cursor, (2, 3), "the twenty-first x sits on the third row");

        // An explicit line break starts a row of its own, and the cursor after
        // it is at the block's second row, column zero.
        let input = "one\ntwo";
        let (rows, cursor) = composer(input, "one\n".len(), 12);
        assert_eq!(rows, vec!["› one".to_string(), "  two".to_string()]);
        assert_eq!(cursor, (1, 2));

        // An empty prompt is one row with the marker, cursor right after it.
        let (rows, cursor) = composer("", 0, 12);
        assert_eq!(rows, vec!["› ".to_string()]);
        assert_eq!(cursor, (0, 2));
    }

    /// Home and End are line gestures on a multi-line draft: the cursor lands
    /// on the edge of its own line, never on the edge of the whole prompt.
    #[test]
    fn a_line_step_stops_at_the_cursor_line() {
        let draft = "first line\nsecond line\nthird";
        let second = draft.find("second").expect("second line");
        let inside = second + "second".len();
        assert_eq!(line_start(draft, inside), second);
        assert_eq!(line_end(draft, inside), second + "second line".len());
        // The first and last lines have no newline on one side, so the draft's
        // own edges are theirs.
        assert_eq!(line_start(draft, 0), 0);
        assert_eq!(line_start(draft, inside), second);
        assert_eq!(line_end(draft, 0), "first line".len());
        assert_eq!(line_end(draft, draft.len()), draft.len());
        // A cursor past the end is clamped rather than panicking: the draft can
        // shrink under a key the frame already read, and the clamp lands on the
        // last line, which is the one the reader was typing into.
        assert_eq!(
            line_start(draft, draft.len() + 40),
            draft.find("third").expect("third line")
        );
    }

    /// A word step lands on a word's edges and steps over the gap after it, so
    /// repeating the key walks the draft instead of stalling on spaces.
    #[test]
    fn a_word_step_lands_on_word_edges_and_skips_the_gap() {
        let draft = "fix the  parser bug";
        let parser = draft.find("parser").expect("parser");
        let bug = draft.find("bug").expect("bug");
        // Backwards: the last word, the one before it, and the one before that
        // - the double space between "the" and "parser" is crossed, not landed
        // in.
        assert_eq!(word_start(draft, draft.len()), bug);
        assert_eq!(word_start(draft, bug), parser);
        assert_eq!(word_start(draft, parser), draft.find("the").expect("the"));
        assert_eq!(word_start(draft, 0), 0, "nothing before the first word");
        // Forwards: past each word, never onto the space following it.
        assert_eq!(word_end(draft, 0), draft.find(' ').expect("space"));
        assert_eq!(word_end(draft, 3), draft.find("the").expect("the") + 3);
        assert_eq!(word_end(draft, 7), parser + "parser".len());
        assert_eq!(word_end(draft, draft.len()), draft.len());
        // A CJK word has no spaces inside it, so the step crosses it whole and
        // the boundary stays on a character edge.
        let cjk = "修复 解析器";
        assert_eq!(word_start(cjk, cjk.len()), "修复 ".len());
        assert!(cjk.is_char_boundary(word_start(cjk, cjk.len())));
    }

    /// Delete and the three kills cut exactly the range their names describe,
    /// which is what makes them predictable without a selection.
    #[test]
    fn the_delete_and_kill_chords_cut_the_range_they_name() {
        // Delete takes the character under the cursor; the cursor stays.
        let mut input = "abc".to_string();
        let mut cursor = 1;
        delete_forward(&mut input, &mut cursor);
        assert_eq!((input.as_str(), cursor), ("ac", 1));
        // At the end there is nothing under the cursor: no edit, no panic.
        let mut cursor = input.len();
        delete_forward(&mut input, &mut cursor);
        assert_eq!((input.as_str(), cursor), ("ac", 2));

        // Ctrl-K cuts to the end of the cursor's line and leaves the lines
        // below it alone.
        let mut input = "one two\nthree\nfour".to_string();
        let mut cursor = 4;
        kill_to_line_end(&mut input, &mut cursor);
        assert_eq!((input.as_str(), cursor), ("one \nthree\nfour", 4));
        // At a line's end the only thing to the right is the newline, so the
        // chord joins the next line rather than doing nothing.
        let mut input = "one\nthree".to_string();
        let mut cursor = 3;
        kill_to_line_end(&mut input, &mut cursor);
        assert_eq!((input.as_str(), cursor), ("onethree", 3));

        // Ctrl-U cuts back to the start of the cursor's line.
        let mut input = "one two\nthree".to_string();
        let mut cursor = 13;
        kill_to_line_start(&mut input, &mut cursor);
        assert_eq!((input.as_str(), cursor), ("one two\n", 8));
        // At a line start it takes the newline before it, joining upwards.
        let mut input = "one two\nthree".to_string();
        let mut cursor = 8;
        kill_to_line_start(&mut input, &mut cursor);
        assert_eq!((input.as_str(), cursor), ("one twothree", 7));
        // An empty draft is not an edit.
        let mut input = String::new();
        let mut cursor = 0;
        kill_to_line_start(&mut input, &mut cursor);
        assert_eq!((input.as_str(), cursor), ("", 0));

        // Ctrl-W takes the word before the cursor, with the space it left.
        let mut input = "fix the parser".to_string();
        let mut cursor = input.len();
        kill_word(&mut input, &mut cursor);
        assert_eq!((input.as_str(), cursor), ("fix the ", 8));
        kill_word(&mut input, &mut cursor);
        assert_eq!((input.as_str(), cursor), ("fix ", 4));
    }

    #[test]
    fn a_tall_prompt_scrolls_but_never_loses_the_cursor() {
        let input = (0..10)
            .map(|line| format!("line{line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let end = input.len();
        let (rows, cursor) = composer(&input, end, 20);
        assert_eq!(rows.len(), MAX_COMPOSER_ROWS, "the block is capped");
        assert_eq!(rows.last().map(String::as_str), Some("  line9"));
        assert_eq!(cursor.0, MAX_COMPOSER_ROWS - 1, "the cursor stays visible");
        // The bottom of the block is where typing happens; the top rows are
        // reachable again by moving the cursor up.
        let (rows, cursor) = composer(&input, 0, 20);
        assert_eq!(rows[0], "› line0");
        assert_eq!(cursor, (0, 2));
    }

    #[test]
    fn the_queue_dock_holds_lines_for_one_session() {
        let one = SessionId::new();
        let two = SessionId::new();
        let mut queue = DraftQueue::default();
        assert!(queue.is_empty());
        assert_eq!(queue.session(), None);
        assert!(queue.push(one, "first"));
        assert!(queue.push(one, "  second\n\nline "));
        // A line for another session could never be sent from this client, so
        // the queue refuses it instead of mixing two inboxes.
        assert!(!queue.push(two, "elsewhere"));
        assert_eq!(queue.session(), Some(one));
        assert_eq!(queue.len(), 2);

        let lines = queue_block(&queue, 60);
        assert_eq!(
            lines,
            vec![
                "queued 2 · sends when the turn ends".to_string(),
                "  1. first".to_string(),
                "  2. second line".to_string(),
            ]
        );

        // Pop takes the oldest line; sending its refused dispatch puts it back
        // at the head, so nothing typed is lost.
        let head = queue.pop().expect("a waiting line");
        assert_eq!(head, "first");
        queue.requeue(head.clone());
        assert_eq!(queue.lines()[0], "first");
        assert_eq!(queue.pop(), Some("first".to_string()));
        assert_eq!(queue.pop(), Some("  second\n\nline ".to_string()));
        assert_eq!(
            queue.session(),
            None,
            "an emptied queue forgets its session"
        );
        assert!(queue.push(two, "new session"), "and may belong to another");

        // One waiting line draws no count header, the desktop's own rule.
        let lines = queue_block(&queue, 60);
        assert_eq!(lines, vec!["  1. new session".to_string()]);

        // Past its cap the dock folds the rest into one counted row. A single
        // waiting line plus the five pushed above is three rows and three
        // folded away.
        for index in 0..MAX_QUEUE_ROWS + 2 {
            assert!(queue.push(two, &format!("line {index}")));
        }
        let lines = queue_block(&queue, 60);
        assert_eq!(lines.len(), MAX_QUEUE_ROWS + 2, "header, rows, one count");
        assert_eq!(
            lines.last().map(String::as_str),
            Some("  … 3 more"),
            "three of the six queued lines fold away"
        );
        assert!(lines.iter().all(|line| line.chars().count() <= 60));

        queue.clear();
        assert!(queue.is_empty(), "clearing drops every waiting line");
        assert_eq!(queue.session(), None);
    }

    #[test]
    fn a_queue_preview_collapses_space_and_keeps_its_budget() {
        assert_eq!(queue_preview("one\n\t two   three"), "one two three");
        let long = "x".repeat(QUEUE_PREVIEW_CHARS + 5);
        let preview = queue_preview(&long);
        assert_eq!(preview.chars().count(), QUEUE_PREVIEW_CHARS + 1);
        assert!(preview.ends_with('…'), "the cut is marked: {preview}");
    }

    #[test]
    fn only_live_executions_hold_their_session() {
        for status in [
            ExecutionStatus::Queued,
            ExecutionStatus::Running,
            ExecutionStatus::WaitingApproval,
        ] {
            assert!(unfinished(status), "{status:?} still holds the session");
        }
        for status in [
            ExecutionStatus::Completed,
            ExecutionStatus::Failed,
            ExecutionStatus::Cancelled,
            ExecutionStatus::Interrupted,
            ExecutionStatus::NeedsIntervention,
        ] {
            assert!(!unfinished(status), "{status:?} will not run again");
        }
    }

    #[test]
    fn every_command_the_menu_offers_is_one_the_terminal_runs() {
        // The list draws its rows from its own catalogue, so a row no parser
        // arm owns would offer the reader a line that answers only with
        // "unknown command".
        for (name, bare) in super::menu::command_rows_for_test() {
            match SlashCommand::parse(name) {
                // A row the parser has never heard of is a promise the list
                // cannot keep, whichever way it is accepted.
                None | Some(SlashCommand::Unknown(_)) => {
                    panic!("the menu offers '{name}', which the parser does not know")
                }
                // A row that runs the moment it is accepted has to run when
                // the same line is typed with no argument: answering with a
                // usage line would leave the reader staring at a command that
                // did nothing.
                Some(SlashCommand::Usage(_)) if bare => {
                    panic!("the menu runs '{name}' on accept, but the parser asks for an argument")
                }
                // A row that leaves the composer open for its argument is
                // allowed to answer with its usage line: `/find` alone names
                // no text to search for, and the row's job was to put the
                // cursor where the argument is typed.
                Some(_) => {}
            }
        }
    }

    #[test]
    fn the_skills_command_reads_the_catalogue_the_palette_ranks() {
        let catalogued = |name: &str, description: &str, user_only: bool| SkillView {
            name: name.to_string(),
            description: description.to_string(),
            user_only,
        };
        let release = catalogued("release", "ship the release", false);
        let grill = catalogued("grill-me", "interrogate the plan", true);
        let quiet = catalogued("quiet", "  ", false);

        // The row is the gesture itself, because that is what the reader types;
        // the manifest's user-only fact is marked instead of being hidden.
        assert_eq!(skill_row(&release), "/release");
        assert_eq!(skill_row(&grill), "/grill-me (user-only)");
        assert_eq!(
            skill_detail(&grill),
            "/grill-me · interrogate the plan · user-invoked only"
        );
        assert_eq!(
            skill_detail(&quiet),
            "/quiet · model may invoke",
            "an empty description draws no empty field"
        );

        // The search reads both halves without case, like the palette field:
        // a reader who remembers the name and one who remembers the sentence
        // both find the row.
        assert!(skill_matches(&release, "RELEASE"));
        assert!(skill_matches(&grill, "plan"));
        assert!(!skill_matches(&grill, "release"));

        // `/skills` is a list, `/skills <text>` a search, `/new` a title prompt.
        assert_eq!(
            SlashCommand::parse("/skills"),
            Some(SlashCommand::Skills(None))
        );
        assert_eq!(
            SlashCommand::parse("/skills grill"),
            Some(SlashCommand::Skills(Some("grill".to_string())))
        );
        assert_eq!(
            SlashCommand::parse("/skills ship the release"),
            Some(SlashCommand::Skills(Some("ship the release".to_string()))),
            "a search is a sentence, not one word"
        );
        assert_eq!(SlashCommand::parse("/new"), Some(SlashCommand::New(None)));
        assert_eq!(
            SlashCommand::parse("/new  ship the parity slice "),
            Some(SlashCommand::New(Some("ship the parity slice".to_string()))),
            "the title keeps its spaces and drops the padding"
        );

        // The Host refuses an empty title, so the terminal never sends one: a
        // blank or whitespace title is the clock's name instead.
        assert_eq!(session_title(Some("  ship it  ")), "ship it");
        assert!(
            session_title(Some("   ")).starts_with("Session "),
            "whitespace is no title"
        );
        assert!(session_title(None).starts_with("Session "));
    }

    /// The rail's rows are the sessions it paints, and only those: a click
    /// past the list, on the separator beside it or in a frame too narrow to
    /// have a rail opens no session.
    #[test]
    fn a_click_names_a_rail_row_only_inside_the_list() {
        // A hundred-column frame draws a twenty-five-cell rail; row 2 is the
        // body's first row, directly under the header and its rule.
        assert_eq!(rail_offset(2, 0, 100, 20), Some(0));
        assert_eq!(
            rail_offset(3, 24, 100, 20),
            Some(1),
            "the list's second row is the frame's third"
        );
        assert_eq!(
            rail_offset(21, 24, 100, 20),
            Some(19),
            "the last body row is still the rail's"
        );
        assert_eq!(
            rail_offset(2, 25, 100, 20),
            None,
            "the separator column is not a session row"
        );
        assert_eq!(rail_offset(1, 3, 100, 20), None, "the header names nothing");
        assert_eq!(
            rail_offset(22, 3, 100, 20),
            None,
            "a row below the body is the composer's"
        );
        // A narrow frame paints no rail at all: its first column is the
        // transcript's.
        assert_eq!(rail_offset(2, 0, 60, 20), None);
        // Before the first frame is painted there is no rail to name.
        assert_eq!(rail_offset(2, 0, 0, 20), None);
    }

    /// The transcript column owns every body column the rail does not: the
    /// separator beside the rail, the header and the rows past the body name
    /// no transcript row.
    #[test]
    fn a_click_names_a_transcript_row_only_inside_the_column() {
        // A hundred-column frame draws a twenty-five-cell rail and a
        // seventy-four-cell transcript after the separator at column 25.
        assert_eq!(transcript_width(100), 74);
        assert_eq!(transcript_offset(2, 26, 100, 20), Some(0));
        assert_eq!(
            transcript_offset(21, 99, 100, 20),
            Some(19),
            "the body's last row is the transcript's"
        );
        assert_eq!(
            transcript_offset(2, 25, 100, 20),
            None,
            "the separator column belongs to neither column"
        );
        assert_eq!(
            transcript_offset(1, 30, 100, 20),
            None,
            "the header is not a row"
        );
        assert_eq!(
            transcript_offset(2, 0, 100, 20),
            None,
            "the rail is not the transcript"
        );
        assert_eq!(
            transcript_offset(22, 30, 100, 20),
            None,
            "a row below the body is the composer's"
        );
        // A narrow frame paints no rail, so its whole row is the transcript's.
        assert_eq!(transcript_width(60), 60);
        assert_eq!(transcript_offset(2, 0, 60, 20), Some(0));
        // A column past the frame's own cells is not painted, so it names
        // nothing even though it falls right of the rail.
        assert_eq!(transcript_offset(2, 100, 100, 20), None);
        // Before the first frame is painted there is no column to name.
        assert_eq!(transcript_offset(2, 0, 0, 20), None);
    }

    /// The hits a click reads are the rows the frame paints: a row that draws
    /// a tool call names that call, and a prose or blank row names nothing.
    #[test]
    fn a_transcript_row_names_the_call_the_frame_paints_there() {
        // Every row of a call names the call, not only its first: the changed
        // patch row is what a reader points at when they want the diff.
        let lines: Vec<(String, Option<usize>)> = vec![
            ("you › go".to_string(), None),
            ("✓ bash · done".to_string(), Some(0)),
            ("  + changed".to_string(), Some(0)),
            ("✓ fs.read · done".to_string(), Some(1)),
        ];
        // A body taller than the document fills its top with blanks, so the
        // newest row still sits directly above the composer.
        let (texts, hits) = window_rows(&lines, 6, 0);
        assert_eq!(texts.len(), 6);
        assert_eq!(hits, [None, None, None, Some(0), Some(0), Some(1)]);
        assert!(
            texts[..2].iter().all(String::is_empty),
            "the fill above the document is blank: {texts:?}"
        );
        assert_eq!(texts[5], "✓ fs.read · done");

        // Scrolling lifts older rows into view; the hits move with them, so a
        // click never opens the call that used to be at that row.
        let (texts, hits) = window_rows(&lines, 6, 1);
        assert_eq!(texts.len(), 6);
        assert_eq!(hits, [None, None, None, None, Some(0), Some(0)]);

        // A full body shows exactly its rows and nothing above them.
        let (texts, hits) = window_rows(&lines, 3, 0);
        assert_eq!(
            texts,
            vec![
                "✓ bash · done".to_string(),
                "  + changed".to_string(),
                "✓ fs.read · done".to_string(),
            ]
        );
        assert_eq!(hits, [Some(0), Some(0), Some(1)]);
        // Scrolling past the top shows only the fill the document has.
        let (texts, hits) = window_rows(&lines, 3, 9);
        assert!(texts.iter().all(String::is_empty), "{texts:?}");
        assert_eq!(hits, [None, None, None]);
    }

    /// The `@` card's session rows resolve back to the snapshot's own row: an
    /// id another client parked since the picker opened opens nothing.
    #[test]
    fn a_completion_row_resolves_to_the_snapshot_it_ranked() {
        let row = |id: SessionId, title: &str| SessionSummary {
            session_id: id,
            title: title.to_string(),
            version: 1,
            updated_at: Utc::now(),
            message_count: 1,
            preview: None,
            active_execution: None,
            pending_approvals: 0,
            pinned: false,
            archived: false,
        };
        let first = SessionId::new();
        let second = SessionId::new();
        let sessions = vec![row(first, "first"), row(second, "second")];
        assert_eq!(session_index(&sessions, &second.to_string()), Some(1));
        assert_eq!(
            session_index(&sessions, &SessionId::new().to_string()),
            None
        );
        assert_eq!(session_index(&sessions, "not-an-id"), None);
        assert_eq!(session_index(&[], &first.to_string()), None);
    }

    /// The card an empty session opens on answers the three questions a
    /// reader has before typing: where the turn runs, what it resolves from and
    /// how to start one. Every row is cut to the frame, so a long title or a
    /// narrow terminal cannot break the box.
    #[test]
    fn an_empty_session_opens_on_the_workspace_card() {
        let root = Path::new("/Users/dev/bos");
        let card = hero_rows(None, Some(root), 80);
        assert_eq!(card.first().map(String::as_str), Some("╭ bos"));
        assert_eq!(card.last().map(String::as_str), Some("╰"));
        assert!(card.iter().any(|row| row.contains("^T starts one")));
        assert!(card.iter().any(|row| row.contains("Enter sends a turn")));
        // A Host serving no folder says so rather than leaving the title blank.
        assert_eq!(
            hero_rows(None, None, 80).first().map(String::as_str),
            Some("╭ no workspace")
        );
        // The card is rows, not a document: a 12-column frame cuts each one.
        for row in hero_rows(None, Some(root), 12) {
            assert!(row.chars().count() <= 12, "row wider than the frame: {row}");
        }
    }

    /// The Host says where turns run and the client believes it: a terminal
    /// launched in one folder and attached to a Host serving another lists the
    /// Host's tree, because that is the tree `@`, `/files` and the shell read.
    #[test]
    fn the_client_adopts_the_workspace_the_host_names() {
        assert_eq!(
            host_workspace("/Users/dev/bos", Some(PathBuf::from("/tmp"))),
            Some(PathBuf::from("/Users/dev/bos")),
            "the Host's root wins over the launching directory"
        );
        assert_eq!(
            host_workspace("/Users/dev/bos", None),
            Some(PathBuf::from("/Users/dev/bos")),
            "an attached client with no directory of its own still knows the tree"
        );
        // A Host with no workspace surface names nothing; the client keeps the
        // directory it opened with rather than losing the tree it can read.
        assert_eq!(
            host_workspace("", Some(PathBuf::from("/tmp"))),
            Some(PathBuf::from("/tmp"))
        );
        assert_eq!(
            host_workspace("   ", Some(PathBuf::from("/tmp"))),
            Some(PathBuf::from("/tmp"))
        );
        assert_eq!(host_workspace("", None), None);
    }

    #[test]
    fn a_session_row_names_the_workspace_directory() {
        let root = Path::new("/Users/dev/bos");
        assert_eq!(workspace_label(Some(root)), "bos");
        assert_eq!(
            workspace_label(None),
            "no workspace",
            "a client that names no directory says so"
        );
    }

    #[test]
    fn the_session_list_numbers_its_rows_and_reads_their_marks() {
        let now = Utc::now();
        let summary = |title: &str, preview: Option<&str>, minutes: i64| SessionSummary {
            session_id: SessionId::new(),
            title: title.to_string(),
            version: 1,
            updated_at: now - chrono::Duration::minutes(minutes),
            message_count: 3,
            preview: preview.map(str::to_string),
            active_execution: None,
            pending_approvals: 0,
            pinned: false,
            archived: false,
        };
        // A row is the number that opens it, its title, the newest message and
        // the age, marked when it is the open session.
        let open = summary("Release", Some("fix the build"), 5);
        assert_eq!(
            session_row(1, &open, true, now),
            "›2. Release — fix the build · 5m · 3 msg"
        );
        assert_eq!(
            session_row(0, &open, false, now),
            " 1. Release — fix the build · 5m · 3 msg"
        );
        // Age uses the coarsest unit that still says which session is recent.
        assert_eq!(age(now, now), "now");
        assert_eq!(age(now - chrono::Duration::hours(2), now), "2h");
        assert_eq!(age(now - chrono::Duration::days(3), now), "3d");

        // The desktop's menu marks a call in flight and counts approvals; a
        // terminal row carries the same facts.
        let mut busy = summary("Fix", None, 90);
        busy.active_execution = Some(ExecutionStatus::WaitingApproval);
        busy.pending_approvals = 2;
        let row = session_row(0, &busy, false, now);
        assert!(
            row.contains("waiting approval") && row.contains("2 approvals"),
            "the row names what waits: {row}"
        );

        // The desktop sidebar stars a pinned chat; a terminal spells the same
        // state as a word beside the age, and an unpinned row never carries it.
        let mut starred = summary("Starred", None, 9);
        starred.pinned = true;
        assert_eq!(
            session_row(0, &starred, false, now),
            " 1. Starred · 9m · 3 msg · pinned"
        );
        assert!(!session_row(0, &open, false, now).contains("pinned"));

        // A filter reads the title and the newest message, without case, the
        // two fields the desktop's search box matches.
        assert!(session_matches(&open, "REL"));
        assert!(session_matches(&open, "build"));
        assert!(!session_matches(&open, "nothing here"));

        // The status row says how many of the workspace's sessions it listed,
        // and folds rows past the tenth.
        let rows = vec!["row".to_string(); 12];
        let listed = session_summary(&rows, 20, None);
        assert!(listed.starts_with("12 of 20 session(s): row"), "{listed}");
        assert!(listed.ends_with("… 2 more"), "{listed}");
        assert!(session_summary(&[], 0, None).contains("no sessions"));
        assert!(session_summary(&[], 4, Some("zzz")).contains("no session matches 'zzz'"));
    }

    #[test]
    fn the_archived_filter_lists_its_rows_and_names_the_way_back() {
        // The filter has its own status line: it answers a different question
        // from `/sessions`, so it says how many of the archived chats a search
        // matched and which verb takes a row back out.
        let rows = vec!["row".to_string(); 12];
        let listed = archived_summary(&rows, 20, None);
        assert!(listed.starts_with("archived: 12 of 20 · row"), "{listed}");
        assert!(listed.ends_with("… 2 more | /sessions unarchive <n> brings one back"));
        assert!(archived_summary(&[], 0, None).contains("no archived sessions"));
        assert!(
            archived_summary(&[], 3, Some("zzz"))
                .contains("no archived session matches 'zzz' · 3 archived")
        );
    }

    #[test]
    fn the_bin_numbers_the_rows_restore_and_purge_address() {
        let now = Utc::now();
        let parked = |title: &str, minutes: i64| SessionSummary {
            session_id: SessionId::new(),
            title: title.to_string(),
            version: 2,
            updated_at: now - chrono::Duration::minutes(minutes),
            message_count: 4,
            preview: None,
            active_execution: None,
            pending_approvals: 0,
            pinned: false,
            archived: false,
        };

        // A bin row is the number `/restore` and `/purge` take, drawn by the
        // same row builder `/sessions` uses, so the two lists cannot disagree
        // about what a row says.
        let rows = vec![session_row(0, &parked("keep me", 5), false, now)];
        assert_eq!(rows[0], " 1. keep me · 5m · 4 msg");
        let listed = trash_summary(&rows, 1);
        assert!(listed.starts_with("bin: 1 parked · "), "{listed}");
        assert!(listed.contains("1. keep me"), "{listed}");
        assert!(listed.contains("/restore <n>"), "{listed}");
        assert!(listed.contains("/purge <n>"), "{listed}");

        // An empty bin says where a chat comes from rather than printing
        // nothing: parking starts at the session list.
        let empty = trash_summary(&[], 0);
        assert!(empty.contains("the bin is empty"), "{empty}");
        assert!(empty.contains("/trash <number>"), "{empty}");

        // The grammar: a bare `/trash` lists, a number parks a `/sessions`
        // row, and restore and purge address a bin row. No number is judged
        // here; the command checks it against the list the Host holds.
        assert_eq!(
            SlashCommand::parse("/trash"),
            Some(SlashCommand::Trash(None))
        );
        assert_eq!(
            SlashCommand::parse("/trash 2"),
            Some(SlashCommand::Trash(Some("2".to_string())))
        );
        assert_eq!(
            SlashCommand::parse("/restore"),
            Some(SlashCommand::Restore(None))
        );
        assert_eq!(
            SlashCommand::parse("/restore 3"),
            Some(SlashCommand::Restore(Some("3".to_string())))
        );
        assert_eq!(
            SlashCommand::parse("/purge 4"),
            Some(SlashCommand::Purge(Some("4".to_string())))
        );
    }

    /// The terminal pane's grammar: `/terminal` takes no argument, because the
    /// program is the reader's own login shell.
    #[test]
    fn the_terminal_command_takes_no_argument() {
        assert_eq!(
            SlashCommand::parse("/terminal"),
            Some(SlashCommand::Terminal)
        );
        assert!(matches!(
            SlashCommand::parse("/terminal zsh"),
            Some(SlashCommand::Usage(_))
        ));
    }

    #[test]
    fn truncation_marks_what_it_drops() {
        assert_eq!(truncate("session one", 6), "sessi…");
        assert_eq!(truncate("abc", 6), "abc");
        assert_eq!(truncate("abc", 0), "");
    }

    /// The arguments one `ask_user` call carries, as the tool wrote them: a
    /// single-select database question, then a multi-select test question.
    const ASKED: &str = r#"{"questions":[{"question":"Which database?","options":["SQLite","Postgres"]},{"question":"Which tests?","options":["unit","integration"],"multi_select":true}]}"#;

    #[test]
    fn the_question_card_reads_the_arguments_the_tool_took() {
        let asked = asked_questions(ASKED).expect("two questions");
        assert_eq!(asked.len(), 2);
        assert_eq!(asked[0].question, "Which database?");
        assert_eq!(asked[0].options, vec!["SQLite", "Postgres"]);
        assert!(!asked[0].multi_select);
        assert!(asked[1].multi_select);

        // Anything that is not an answerable question is no card at all: a call
        // with no arguments, arguments that are not JSON, an empty list, and a
        // question with one option - which the tool itself refuses.
        assert!(asked_questions("{}").is_none());
        assert!(asked_questions("not json").is_none());
        assert!(asked_questions("{\"questions\":[]}").is_none());
        assert!(
            asked_questions("{\"questions\":[{\"question\":\"one?\",\"options\":[\"only\"]}]}")
                .is_none(),
            "a single option is not a choice"
        );

        // `/questions` is the way back to a card Escape put aside, and it takes
        // no argument the way every other listing does.
        assert_eq!(
            SlashCommand::parse("/questions"),
            Some(SlashCommand::Questions)
        );
        assert!(matches!(
            SlashCommand::parse("/questions 3"),
            Some(SlashCommand::Usage(_))
        ));
    }

    #[test]
    fn the_card_walks_options_toggles_and_composes_the_desktops_answer() {
        let asked = asked_questions(ASKED).expect("two questions");
        let mut card = QuestionCard::new(7, asked);
        assert_eq!(card.current().expect("a step").question, "Which database?");
        // The highlight clamps at both ends: the first press at the top stays
        // put, and the last option is the end of the walk.
        card.move_by(-1);
        assert_eq!(card.highlight, 0);
        card.move_by(5);
        assert_eq!(card.highlight, 1);
        // A digit puts the highlight on the option it numbers, and a digit past
        // the last option moves nothing rather than guessing a neighbour.
        card.jump('1');
        assert_eq!(card.highlight, 0);
        card.jump('9');
        assert_eq!(card.highlight, 0);
        card.jump('0');
        assert_eq!(card.highlight, 0, "digits are one-based");
        // A single-select pick replaces the step's one pick and moves on.
        assert_eq!(card.pick(), Some(false));
        assert_eq!(card.picks[0], vec!["SQLite"]);
        assert!(!card.advance(), "a second step remains");
        assert_eq!(card.highlight, 0, "a new step starts at its first option");
        // A multi-select pick toggles the option in and out again.
        assert_eq!(card.pick(), Some(true));
        assert_eq!(card.picks[1], vec!["unit"]);
        card.move_by(1);
        assert_eq!(card.pick(), Some(true));
        assert_eq!(card.picks[1], vec!["unit", "integration"]);
        card.jump('1');
        assert_eq!(card.pick(), Some(true));
        assert_eq!(card.picks[1], vec!["integration"], "the pick toggles out");
        assert!(card.advance(), "the last step is the end");
        assert_eq!(
            card.answer(),
            "Which database?: SQLite\nWhich tests?: integration",
            "the message is the desktop's own shape"
        );
    }

    #[test]
    fn a_question_stays_pending_until_the_reader_answers_it() {
        let mut asked = call("ask_user", ToolCallStatus::Completed, ASKED, Some("{}"));
        asked.started_at = Utc::now() - chrono::Duration::seconds(30);
        let mut session = projection(vec![asked.clone()]);
        let (index, questions) = pending_question(&session).expect("the call is pending");
        assert_eq!(index, 0);
        assert_eq!(questions.len(), 2);

        // A reader turn after the call is the answer: the card settles, exactly
        // as the desktop's card settles on the reader's next message.
        let mut answered = message(MessageRole::User, "Postgres", None);
        answered.at = asked.started_at + chrono::Duration::seconds(1);
        session.messages = vec![answered];
        assert!(pending_question(&session).is_none());

        // The assistant text the turn committed after the question is not an
        // answer: no reader spoke, so the card stays.
        let mut still = projection(vec![asked.clone()]);
        let mut said = message(MessageRole::Assistant, "Which database?", None);
        said.at = asked.started_at + chrono::Duration::seconds(1);
        still.messages = vec![said];
        assert!(pending_question(&still).is_some());

        // A failed call, or a session whose newest call is another tool.
        let failed = projection(vec![call("ask_user", ToolCallStatus::Failed, ASKED, None)]);
        assert!(pending_question(&failed).is_none());
        let other = projection(vec![call(
            "update_plan",
            ToolCallStatus::Completed,
            "{}",
            None,
        )]);
        assert!(pending_question(&other).is_none());

        // The newest ask_user call is the one a reader sees: an older question
        // already answered behind it cannot come back.
        let mut both = projection(vec![
            call("ask_user", ToolCallStatus::Completed, ASKED, Some("{}")),
            asked,
        ]);
        both.tool_calls[0].started_at = Utc::now() - chrono::Duration::seconds(60);
        let (index, _) = pending_question(&both).expect("the newer call asks");
        assert_eq!(index, 1);
    }

    #[test]
    fn the_card_draws_the_step_and_the_options_where_the_answer_is_typed() {
        let asked = asked_questions(ASKED).expect("two questions");
        let mut card = QuestionCard::new(0, asked);
        let rows = question_lines(&card, 60);
        assert_eq!(rows[0], "questions · 1/2");
        assert!(rows[1].starts_with("? Which database?"), "{}", rows[1]);
        assert_eq!(rows[2], "▸    1 · SQLite", "the highlight leads the option");
        assert_eq!(rows[3], "     2 · Postgres");
        let legend = rows.last().expect("a legend row");
        assert!(
            legend.contains("Enter answers") && legend.contains("Esc dismisses"),
            "{legend}"
        );

        // A multi-select step marks every pick it holds and says how to move on.
        card.step = 1;
        let rows = question_lines(&card, 60);
        assert!(
            rows[0].contains("2/2") && rows[0].contains("multi-select"),
            "{}",
            rows[0]
        );
        assert_eq!(rows[2], "▸[ ] 1 · unit");
        assert_eq!(rows[3], " [ ] 2 · integration");
        let legend = rows.last().expect("a legend row");
        assert!(legend.contains("Ctrl-J moves on"), "{legend}");
        assert!(rows.iter().all(|row| row.chars().count() <= 60));

        // A narrow frame truncates a row instead of wrapping one the card owns,
        // so the options keep their rows in a split terminal.
        let narrow = question_lines(&card, 20);
        assert!(
            narrow.iter().all(|row| row.chars().count() <= 20),
            "{narrow:?}"
        );
    }

    /// One call as the Host projects it, with the fields a detail pane reads.
    fn call(tool: &str, status: ToolCallStatus, args: &str, output: Option<&str>) -> ToolCallView {
        ToolCallView {
            execution_id: None,
            step: 1,
            tool: tool.to_string(),
            summary: format!("{tool} ran"),
            args: args.to_string(),
            status,
            output: output.map(str::to_string),
            duration_ms: Some(12),
            started_at: Utc::now(),
            finished_at: None,
        }
    }

    /// A projection holding exactly the calls the test cares about.
    fn projection(calls: Vec<ToolCallView>) -> SessionProjection {
        SessionProjection {
            session_id: SessionId::new(),
            version: 1,
            title: "Detail".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            messages: Vec::new(),
            tool_calls: calls,
            plan: Vec::new(),
            executions: Vec::new(),
            approvals: Vec::new(),
            config: SessionConfig::default(),
            compaction: None,
            goal: None,
            feedback: None,
            trashed: false,
            pinned: false,
            archived: false,
        }
    }

    #[test]
    fn detail_pane_names_the_call_and_keeps_its_title_while_scrolling() {
        let session = projection(vec![call(
            "bash",
            ToolCallStatus::Completed,
            "{\"command\":\"cargo test\"}",
            Some("ok\nsecond line"),
        )]);
        let rows = detail_pane(Some(&session), 0, 90, 10, 0);
        assert!(
            rows[0].starts_with("detail 1/1 · bash · done · 12ms")
                && rows[0].ends_with("Esc closes"),
            "the title names the call, its state and the way out: {}",
            rows[0]
        );
        let body = rows[1..].join("\n");
        assert!(
            body.contains("args"),
            "the body shows the arguments: {body}"
        );
        assert!(body.contains("cargo test"), "{body}");
        assert!(body.contains("output"), "{body}");
        assert!(body.contains("second line"), "{body}");
        // Scrolling past the end of a body shorter than the pane changes nothing:
        // the tail is already in view, so the title stays on row zero.
        let scrolled = detail_pane(Some(&session), 0, 90, 10, 99);
        assert_eq!(scrolled[0], rows[0], "the title survives any scroll");
    }

    #[test]
    fn detail_pane_folds_long_output_and_reports_a_missing_call() {
        let long = "row\n".repeat(DETAIL_ROWS + 5);
        let session = projection(vec![call(
            "bash",
            ToolCallStatus::Running,
            "{}",
            Some(&long),
        )]);
        let rows = detail_pane(Some(&session), 0, 40, DETAIL_ROWS, 10);
        assert!(
            rows.iter().any(|row| row.contains("more row(s)")),
            "a body past the cap folds into one counted row"
        );
        // Position one past the last call is not a call this session made.
        let missing = detail_pane(Some(&session), 4, 40, 3, 0);
        assert!(
            missing[0].contains("no longer in the projection"),
            "{}",
            missing[0]
        );
        assert_eq!(missing.len(), 3, "the pane still fills the rows it owns");
        // No session at all is the same answer rather than a panic.
        assert!(detail_pane(None, 0, 20, 2, 0)[0].contains("no longer"));
    }

    #[test]
    fn indented_keeps_source_indentation_and_splits_wide_rows() {
        let rows = indented("  ", "a\n  b", 10);
        assert_eq!(rows, vec!["  a".to_string(), "    b".to_string()]);
        let wide = indented("  ", &"x".repeat(20), 10);
        assert!(wide.iter().all(|row| row.chars().count() <= 10), "{wide:?}");
        assert_eq!(wide.len(), 3, "16 cells per chunk carry 20 x's: {wide:?}");
    }

    /// One projected message, carrying the usage its turn reported.
    fn message(role: MessageRole, text: &str, usage: Option<TurnUsage>) -> MessageView {
        MessageView {
            message_id: crate::runtime::ids::MessageId::new(),
            role,
            text: text.to_string(),
            at: Utc::now(),
            execution_id: None,
            compaction: false,
            usage,
        }
    }

    #[test]
    fn the_usage_chip_and_the_exact_readout_read_the_turns_that_reported() {
        let mut session = projection(Vec::new());
        // `/usage` parses bare and refuses an argument the way every other
        // no-argument command does.
        assert_eq!(SlashCommand::parse("/usage"), Some(SlashCommand::UsageShow));
        assert!(matches!(
            SlashCommand::parse("/usage 3"),
            Some(SlashCommand::Usage(_))
        ));

        session.messages = vec![
            message(MessageRole::User, "question", None),
            message(
                MessageRole::Assistant,
                "answer",
                Some(TurnUsage {
                    prompt_tokens: 8_000,
                    cached_tokens: 3_000,
                    completion_tokens: 200,
                }),
            ),
            message(
                MessageRole::Assistant,
                "answer 2",
                Some(TurnUsage {
                    prompt_tokens: 6_000,
                    // More cached tokens than prompt tokens is a provider
                    // mistake; the desktop's sum clamps it and so does this.
                    cached_tokens: 6_500,
                    completion_tokens: 100,
                }),
            ),
        ];
        let total = session_usage(&session).expect("two turns reported usage");
        assert_eq!(total.prompt_tokens, 14_000);
        assert_eq!(total.completion_tokens, 300);
        assert_eq!(total.cached_tokens, 9_000, "the second turn is clamped");
        assert_eq!(total.uncached_input(), 5_000);
        assert_eq!(total.total(), 14_300);
        assert_eq!(
            usage_chip(&total),
            "usage 14.3K · 64% cached",
            "the chip carries the pill's compact total and cache share"
        );
        // A turn that read no cache at all is a 0% hit, the figure the desktop
        // pill also prints; only a prompt with no tokens has no share to claim.
        assert_eq!(
            usage_chip(&TurnUsage {
                prompt_tokens: 900,
                cached_tokens: 0,
                completion_tokens: 40,
            }),
            "usage 940 · 0% cached"
        );
        assert_eq!(
            usage_chip(&TurnUsage {
                prompt_tokens: 0,
                cached_tokens: 0,
                completion_tokens: 40,
            }),
            "usage 40",
            "no prompt input is unknown, not zero"
        );

        // A session whose turns reported nothing has no total to show, and the
        // header says so instead of drawing a zero.
        session.messages = vec![message(MessageRole::User, "question", None)];
        assert!(session_usage(&session).is_none());
    }

    /// One MCP server as the Host's catalogue would state it.
    fn server(name: &str, tools: &[&str], error: Option<&str>) -> McpServerView {
        McpServerView {
            name: name.to_string(),
            transport: format!("npx {name}"),
            enabled: true,
            extension: false,
            tools: tools.iter().map(|tool| tool.to_string()).collect(),
            error: error.map(str::to_string),
        }
    }

    #[test]
    fn the_mcp_grammar_is_a_bare_list_or_one_server() {
        assert_eq!(SlashCommand::parse("/mcp"), Some(SlashCommand::Mcp(None)));
        assert_eq!(
            SlashCommand::parse("/mcp docs"),
            Some(SlashCommand::Mcp(Some("docs".to_string())))
        );
        assert!(matches!(
            SlashCommand::parse("/mcp docs extra"),
            Some(SlashCommand::Usage(_))
        ));
    }

    /// The roster names each server's state in the terms the desktop pane
    /// separates: the tools it offers, switched off, or the reason it failed.
    #[test]
    fn an_mcp_row_states_tools_disabled_or_the_reason() {
        assert_eq!(
            mcp_state(&server("docs", &["docs_read", "docs_list"], None)),
            "2 tool(s)"
        );
        assert_eq!(
            mcp_row(&server("docs", &["docs_read"], None)),
            "docs 1 tool(s)"
        );
        assert_eq!(
            mcp_state(&server("empty", &[], None)),
            "connected, no tools"
        );
        let mut off = server("off", &[], None);
        off.enabled = false;
        assert_eq!(
            mcp_state(&off),
            "disabled",
            "a switched-off row is not a failure"
        );
        assert_eq!(
            mcp_state(&server("broken", &[], Some("spawn failed"))),
            "not connected: spawn failed",
            "the failure reason is what a reader acts on"
        );
    }

    /// A server in full keeps its source and its tools, and says the transport
    /// it runs - which is the launch line, never a credential.
    #[test]
    fn an_mcp_detail_names_source_transport_and_tools() {
        let mut declared = server("docs", &["docs_read", "docs_list"], None);
        declared.transport = "npx -y @docs/server".to_string();
        assert_eq!(
            mcp_detail(&declared),
            "mcp docs · settings · npx -y @docs/server · 2 tool(s) · \
             tools: docs_read, docs_list"
        );
        let mut extension = server("pkg", &[], None);
        extension.extension = true;
        extension.enabled = false;
        assert_eq!(
            mcp_detail(&extension),
            "mcp pkg · extension · npx pkg · disabled"
        );
    }

    /// The Memory pane is the desktop pane's terminal form: the same records,
    /// the same lifecycle words, the same recall explanation.
    #[test]
    fn the_memory_pane_lists_the_store_the_desktop_pane_browses() {
        assert_eq!(
            SlashCommand::parse("/memory"),
            Some(SlashCommand::Memory(None))
        );
        assert_eq!(
            SlashCommand::parse("/memory release checklist"),
            Some(SlashCommand::Memory(Some("release checklist".to_string()))),
            "a search is a sentence, not one word"
        );

        let now = Utc::now();
        let record = |id: &str, content: &str, kind: &str, lifecycle: &str| MemoryView {
            id: id.to_string(),
            content: content.to_string(),
            kind: kind.to_string(),
            scope: "workspace".to_string(),
            scope_id: "bos".to_string(),
            lifecycle: lifecycle.to_string(),
            source: "agent".to_string(),
            revision: 3,
            updated_at: now - chrono::Duration::minutes(90),
            score: None,
            reasons: String::new(),
        };

        // A listed record is its text plus one footer: kind, the scope and the
        // shortened scope id, provenance, revision and age - the desktop
        // card's pills and its `rev N · age` row in one terminal line.
        let kept = record(
            "m1",
            "the release checklist lives in docs/release.md",
            "fact",
            "active",
        );
        let rows = memory_row(&kept, 80, now);
        assert_eq!(rows.len(), 2, "text, then one footer row");
        assert!(rows[0].starts_with("• the release checklist"));
        assert_eq!(rows[1].trim(), "fact · workspace:bos · agent · rev 3 · 1h");

        // A record that no longer steers a turn names its lifecycle, because
        // that is what a reader browsing history is looking for.
        let superseded = record("m2", "prefer tabs over spaces", "preference", "superseded");
        let history = memory_row(&superseded, 80, now);
        assert!(history[1].contains("superseded"));

        // A recall hit carries its score and named contributions between the
        // text and the footer, as the desktop hit card does.
        let hit = MemoryView {
            score: Some(0.87),
            reasons: "keyword 0.80 · recency 0.07".to_string(),
            ..kept.clone()
        };
        let hit_rows = memory_row(&hit, 80, now);
        assert!(hit_rows[1].contains("score 0.87"));
        assert!(hit_rows[1].contains("keyword 0.80"));

        // The status row counts what the pane shows; the pane itself states
        // the store's lifecycle rather than inferring it from an empty list.
        let ready = MemoryReport {
            memories: vec![kept.clone()],
            state: "ready".to_string(),
            error: None,
        };
        assert_eq!(
            memory_summary(&ready, None),
            "memory · 1 record(s) · Esc closes · Up/Down scrolls"
        );
        let empty = MemoryReport::default();
        assert_eq!(
            memory_summary(&empty, Some("widgets")),
            "memory · no hit for \"widgets\" · Esc closes · Up/Down scrolls"
        );
        let disabled = MemoryReport {
            state: "disabled".to_string(),
            ..MemoryReport::default()
        };
        assert!(memory_summary(&disabled, None).contains("memory disabled"));
        assert!(
            memory_body(&disabled, None, 80, now)[0].contains("switched off"),
            "switched off is stated, not implied by an empty list"
        );
        assert!(memory_body(&empty, Some("widgets"), 80, now)[0].contains("no record matched"));
        // A store that refused a search says why, instead of answering "none".
        let skipped = MemoryReport {
            memories: Vec::new(),
            state: "empty".to_string(),
            error: Some("the recall index is not ready".to_string()),
        };
        assert!(
            memory_body(&skipped, Some("widgets"), 80, now)
                .iter()
                .any(|line| line.contains("recall skipped · the recall index is not ready"))
        );
    }

    /// The Settings pane is the desktop settings dialog's read side: the rail's
    /// sections, each row's effective value and a one-word provenance, so a
    /// terminal shows what the next turn resolves from rather than a second
    /// read of the settings files.
    #[test]
    fn the_settings_pane_lists_the_rail_the_desktop_dialog_edits() {
        assert_eq!(
            SlashCommand::parse("/settings"),
            Some(SlashCommand::Settings(None))
        );
        assert_eq!(
            SlashCommand::parse("/settings model"),
            Some(SlashCommand::Settings(Some("model".to_string())))
        );
        assert!(
            matches!(
                SlashCommand::parse("/settings model extra"),
                Some(SlashCommand::Usage(_))
            ),
            "a section name is one word"
        );

        // `/set` is the write side of the same rail: a bare command opens it,
        // a key alone clears what the Host allows to be emptied, and the value
        // is the rest of the line so a verify command keeps its spaces.
        assert_eq!(
            SlashCommand::parse("/set"),
            Some(SlashCommand::Set {
                key: None,
                value: None,
            })
        );
        assert_eq!(
            SlashCommand::parse("/set   temperature   0.4"),
            Some(SlashCommand::Set {
                key: Some("temperature".to_string()),
                value: Some("0.4".to_string()),
            })
        );
        assert_eq!(
            SlashCommand::parse("/set verify-command cargo test --all"),
            Some(SlashCommand::Set {
                key: Some("verify-command".to_string()),
                value: Some("cargo test --all".to_string()),
            })
        );
        assert_eq!(
            SlashCommand::parse("/set temperature"),
            Some(SlashCommand::Set {
                key: Some("temperature".to_string()),
                value: None,
            }),
            "a key with no value is the clear form"
        );

        let row = |label: &str, value: &str, source: &str| SettingsRowView {
            label: label.to_string(),
            value: value.to_string(),
            source: source.to_string(),
            key: None,
        };
        // The row a write can address carries the key that addresses it, which
        // is the same fact the pane draws as a star.
        let writable = |key: &str, label: &str, value: &str, source: &str| SettingsRowView {
            key: Some(key.to_string()),
            ..row(label, value, source)
        };
        let report = SettingsReport {
            sections: vec![
                SettingsSectionView {
                    section: "model".to_string(),
                    rows: vec![
                        row("provider", "anthropic", "settings"),
                        row("model", "claude-sonnet-4", "workspace"),
                    ],
                },
                SettingsSectionView {
                    section: "web-search".to_string(),
                    rows: vec![writable(
                        "web-search-base-url",
                        "endpoint",
                        "https://search",
                        "default",
                    )],
                },
            ],
            state: "ready".to_string(),
            error: None,
        };
        let pane = SettingsPane {
            report,
            section: None,
            scroll: 0,
        };
        let body = settings_body(&pane.report, None, 48);
        assert!(body[0].starts_with("MODEL · 2 row(s)"), "{body:?}");
        assert!(
            body.iter()
                .any(|line| line.contains("anthropic") && line.contains("(settings)")),
            "the value carries where it came from: {body:?}"
        );
        assert!(
            body.iter()
                .any(|line| line.contains("claude-sonnet-4") && line.contains("(workspace)")),
            "a workspace override is distinguishable: {body:?}"
        );
        // The star is the pane's only write affordance: a row that carries a
        // key is a row `/set` can address, and a row that carries none is not.
        let writable_row = body
            .iter()
            .find(|line| line.contains("https://search"))
            .expect("the endpoint row");
        assert!(writable_row.starts_with("* "), "{writable_row:?}");
        let read_only = body
            .iter()
            .find(|line| line.contains("anthropic"))
            .expect("the provider row");
        assert!(read_only.starts_with("  "), "{read_only:?}");
        assert_eq!(editable_rows(&pane.report).len(), 1);
        // The status after a write names the row the Host reports now, not the
        // text that was typed, and says the write reaches the next turn.
        let written = setting_summary(&pane.report, "web-search-base-url");
        assert!(written.contains("https://search"), "{written}");
        assert!(written.contains("(default)"), "{written}");
        assert!(written.contains("the next turn uses it"), "{written}");
        // A key the Host reports no row for still says the write landed instead
        // of pretending the row is there.
        let absent = setting_summary(&pane.report, "temperature");
        assert!(absent.contains("the Host lists no row for it"), "{absent}");
        assert!(settings_summary(&pane.report, None).contains("2 section(s)"));
        let title = settings_title(&pane);
        assert!(title.contains("3 row(s)"), "{title}");
        let rows = settings_pane(&pane, 48, 4);
        assert_eq!(rows.len(), 4, "the pane fills the body exactly");
        assert_eq!(rows[0], truncate(&title, 48));

        // Naming a section narrows the rail; a name the Host does not have is
        // answered with the names it does, not an empty frame.
        let model = settings_body(&pane.report, Some("model"), 48);
        assert_eq!(model.len(), 3, "{model:?}");
        let unknown = settings_summary(&pane.report, Some("theme"));
        assert!(
            unknown.starts_with("no settings section \"theme\""),
            "{unknown}"
        );
        let mut unknown_pane = SettingsPane {
            report: pane.report.clone(),
            section: Some("theme".to_string()),
            scroll: 0,
        };
        assert!(
            settings_title(&unknown_pane).contains("not a section"),
            "the refusal reaches the title: {}",
            settings_title(&unknown_pane)
        );
        // The Host's own refusal carries the names it has and returns no
        // section, so the body states that one reason rather than two.
        unknown_pane.report = SettingsReport {
            sections: Vec::new(),
            state: "ready".to_string(),
            error: Some("no settings section 'theme'; known: model, web-search".to_string()),
        };
        let refused = settings_body(&unknown_pane.report, Some("theme"), 200);
        let refused = refused.join(" ");
        assert!(
            refused.contains("no settings section 'theme'"),
            "{refused:?}"
        );
        assert!(refused.contains("known: model, web-search"), "{refused:?}");
        assert!(
            settings_summary(&unknown_pane.report, Some("theme"))
                .starts_with("no settings section"),
            "the Host's refusal is the status line's first words"
        );
        assert!(
            settings_body(&pane.report, Some("theme"), 48)
                .join(" ")
                .contains("model, web-search"),
            "the refusal lists the sections that exist"
        );

        // A Host that carries no settings says so rather than showing nothing.
        let unsupported = SettingsReport {
            state: "unsupported".to_string(),
            ..SettingsReport::default()
        };
        assert!(settings_summary(&unsupported, None).contains("unsupported"));
        assert!(
            settings_body(&unsupported, None, 48)
                .join(" ")
                .contains("resolves its settings as unsupported")
        );
    }

    /// The Files pane is the desktop's Workspace browser in the terminal: one
    /// directory at a time, one file's text, and the Host's own refusals rather
    /// than an empty listing standing in for them.
    #[test]
    fn the_files_pane_browses_the_workspace_the_desktop_browser_shows() {
        assert_eq!(
            SlashCommand::parse("/files"),
            Some(SlashCommand::Files(None))
        );
        assert_eq!(
            SlashCommand::parse("/files docs/rfc"),
            Some(SlashCommand::Files(Some("docs/rfc".to_string()))),
            "a path is free text, not one word"
        );

        let entry = |name: &str, dir: bool, size: u64| WorkspaceEntryView {
            name: name.to_string(),
            dir,
            size,
        };

        // The cursor is the terminal's pointer: the highlighted row is marked and
        // the rest keep the same indent, so a listing never jitters as it moves.
        let directory = files_row(&entry("src", true, 0), true, 40);
        assert!(directory.starts_with("› src/"), "{directory:?}");
        let file = files_row(&entry("Cargo.toml", false, 2048), false, 40);
        assert!(file.starts_with("  Cargo.toml"), "{file:?}");
        assert!(file.ends_with("2.0 KB"), "{file:?}");
        assert_eq!(file.chars().count(), 40, "the size holds the right edge");

        // A raw byte count becomes the unit a reader thinks in, by the desktop
        // file manager's own rule.
        assert_eq!(file_size_label(0, true), "");
        assert_eq!(file_size_label(37, false), "37 B");
        assert_eq!(file_size_label(1536, false), "1.5 KB");
        assert_eq!(file_size_label(5 * 1024 * 1024, false), "5.0 MB");

        // A directory is a listing: its rows, its cursor and how to move.
        let listing = WorkspaceReport {
            root: "/Users/me/bos".to_string(),
            path: "docs".to_string(),
            state: "dir".to_string(),
            entries: vec![entry("rfc", true, 0), entry("notes.md", false, 512)],
            ..WorkspaceReport::default()
        };
        assert!(files_summary(&listing).contains("docs"));
        assert!(files_summary(&listing).contains("2 row(s)"));
        let rows = files_body(&listing, 1, 40);
        assert_eq!(rows.len(), 2);
        assert!(rows[1].starts_with("› notes.md"), "{rows:?}");

        // A file is its own text, and a cut head says so instead of ending
        // silently where the Host stopped reading.
        let document = WorkspaceReport {
            root: "/Users/me/bos".to_string(),
            path: "README.md".to_string(),
            state: "file".to_string(),
            content: Some("# bos\n\nbuild me".to_string()),
            ..WorkspaceReport::default()
        };
        let text = files_body(&document, 0, 40);
        assert_eq!(text[0], "# bos");
        let pane = FilesPane {
            report: document.clone(),
            selection: 0,
            scroll: 0,
        };
        assert!(
            files_title(&pane).contains("3 line(s)"),
            "the title counts what the pane holds: {}",
            files_title(&pane)
        );
        let head = WorkspaceReport {
            truncated: true,
            ..document
        };
        assert!(
            files_body(&head, 0, 40)
                .iter()
                .any(|row| row.contains("continues past"))
        );

        // A path the Host refused is stated with its reason, and the root has a
        // name of its own rather than an empty one.
        let refused = WorkspaceReport {
            path: "../etc".to_string(),
            state: "refused".to_string(),
            error: Some("that path climbs out of the workspace".to_string()),
            ..WorkspaceReport::default()
        };
        assert!(files_summary(&refused).contains("refused"));
        assert!(files_body(&refused, 0, 40).join(" ").contains("climbs out"));
        assert_eq!(files_place(&refused), "../etc");
        assert_eq!(files_place(&WorkspaceReport::default()), "/");

        // Descending and climbing back is one path operation each, and the Host
        // is what resolves it: the client only names the child it was given.
        assert_eq!(join_path("", "src"), "src");
        assert_eq!(join_path("docs", "rfc"), "docs/rfc");
        assert_eq!(parent_path("docs/rfc"), "docs");
        assert_eq!(parent_path("README.md"), "");
    }

    /// The Diff pane is the desktop Diff tab in the terminal: the working
    /// copy's changed files with the letters the Files tree badges them with,
    /// and one file's unified patch when a row is opened or a path is typed.
    /// The pane never computes a diff of its own.
    #[test]
    fn the_diff_pane_lists_the_changes_and_draws_one_comparison() {
        assert_eq!(SlashCommand::parse("/diff"), Some(SlashCommand::Diff(None)));
        assert_eq!(
            SlashCommand::parse("/diff src/app.rs"),
            Some(SlashCommand::Diff(Some("src/app.rs".to_string()))),
            "a path is free text, not one word"
        );

        let change = |path: &str, letter: &str, label: &str| WorkspaceChangeView {
            path: path.to_string(),
            letter: letter.to_string(),
            label: label.to_string(),
        };

        // A change row is the cursor, the shared status letter and the path.
        let modified = diff_row(&change("src/app.rs", "M", "Modified"), true, 40);
        assert!(modified.starts_with("› M src/app.rs"), "{modified:?}");
        let added = diff_row(&change("src/new.rs", "A", "Added"), false, 40);
        assert!(added.starts_with("  A src/new.rs"), "{added:?}");
        // A VCS that classified nothing still gets a marker rather than a gap.
        assert!(diff_row(&change("notes.txt", "", ""), false, 40).contains("? notes.txt"));

        // The list is every changed path, and its summary names the VCS.
        let listing = WorkspaceDiffView {
            root: "/Users/me/bos".to_string(),
            state: "listed".to_string(),
            vcs: "git".to_string(),
            changes: vec![
                change("src/app.rs", "M", "Modified"),
                change("src/new.rs", "A", "Added"),
            ],
            ..WorkspaceDiffView::default()
        };
        assert!(!diff_is_patch(&listing));
        assert!(!diff_refusal(&listing));
        assert!(diff_summary(&listing).contains("git"));
        assert!(diff_summary(&listing).contains("2 changed"));
        let rows = diff_body(&listing, 1, 40);
        assert_eq!(rows.len(), 2);
        assert!(rows[1].starts_with("› A src/new.rs"), "{rows:?}");
        assert!(
            diff_title(&DiffPane {
                view: listing.clone(),
                selection: 1,
                scroll: 0,
            })
            .contains("Enter opens"),
            "a listing says how a row is opened"
        );

        // One path is its comparison, counted the way the pane counts it:
        // the file markers are headers, not added lines.
        let patch = WorkspaceDiffView {
            state: "patch".to_string(),
            vcs: "git".to_string(),
            changes: listing.changes.clone(),
            path: "src/app.rs".to_string(),
            patch: Some(
                "diff --git a/src/app.rs b/src/app.rs\n--- a/src/app.rs\n+++ b/src/app.rs\n@@ -1 +1 @@\n-old\n+new".to_string(),
            ),
            ..WorkspaceDiffView::default()
        };
        assert!(diff_is_patch(&patch));
        assert_eq!(diff_counts(&patch), (1, 1), "the markers are not counted");
        let body = diff_body(&patch, 0, 60);
        assert!(body.iter().any(|row| row == "+new"));
        assert!(body.iter().any(|row| row == "-old"));
        assert!(diff_summary(&patch).contains("+1 -1"));
        assert!(
            diff_title(&DiffPane {
                view: patch.clone(),
                selection: 0,
                scroll: 0,
            })
            .contains("Enter list")
        );
        assert_eq!(diff_place(&patch), "src/app.rs");

        // A cut patch says so instead of ending where the Host stopped.
        let truncated = WorkspaceDiffView {
            truncated: true,
            ..patch.clone()
        };
        assert!(
            diff_body(&truncated, 0, 60)
                .iter()
                .any(|row| row.contains("continues past"))
        );

        // A clean file and a clean working copy are answers, not empties, and
        // they name what matched its baseline.
        let clean_file = WorkspaceDiffView {
            state: "clean".to_string(),
            vcs: "git".to_string(),
            path: "src/app.rs".to_string(),
            ..WorkspaceDiffView::default()
        };
        assert!(
            diff_body(&clean_file, 0, 40)
                .join(" ")
                .contains("src/app.rs matches its baseline")
        );
        let clean = WorkspaceDiffView {
            state: "clean".to_string(),
            vcs: "git".to_string(),
            ..WorkspaceDiffView::default()
        };
        assert!(
            diff_body(&clean, 0, 40)
                .join(" ")
                .contains("working copy matches its baseline")
        );
        assert_eq!(diff_place(&WorkspaceDiffView::default()), "/");

        // The Host's refusals are stated with their reason rather than read as
        // an empty list: a root that is no checkout compared nothing.
        let unknown = WorkspaceDiffView {
            root: "/Users/me/bos".to_string(),
            state: "unknown".to_string(),
            error: Some("the workspace is not a jj or git checkout".to_string()),
            ..WorkspaceDiffView::default()
        };
        assert!(diff_refusal(&unknown));
        assert!(diff_summary(&unknown).contains("not a jj or git checkout"));
        assert!(
            diff_body(&unknown, 0, 80)
                .join(" ")
                .contains("not a jj or git checkout")
        );
        assert!(
            matches!(
                WorkspaceDiffView {
                    state: "refused".to_string(),
                    ..WorkspaceDiffView::default()
                },
                view if diff_refusal(&view)
            ),
            "a refused path is a refusal, not a clean answer"
        );
    }

    /// The listing window follows the cursor and the patch window follows its
    /// own offset, with the title staying on row one of either.
    #[test]
    fn the_diff_pane_windows_its_rows_under_a_stable_title() {
        let change = |index: usize| WorkspaceChangeView {
            path: format!("file{index:02}.rs"),
            letter: "M".to_string(),
            label: "Modified".to_string(),
        };
        let listing = WorkspaceDiffView {
            root: "/Users/me/bos".to_string(),
            state: "listed".to_string(),
            vcs: "git".to_string(),
            changes: (0..20).map(change).collect(),
            ..WorkspaceDiffView::default()
        };
        let top = DiffPane {
            view: listing.clone(),
            selection: 0,
            scroll: 0,
        };
        let rows = diff_pane(&top, 40, 6);
        assert_eq!(rows.len(), 6, "the pane fills the body it was given");
        assert!(rows[0].starts_with("diff · "), "{rows:?}");
        assert!(rows[1].starts_with("› M file00.rs"), "{rows:?}");

        let bottom = DiffPane {
            view: listing,
            selection: 19,
            scroll: 0,
        };
        let rows = diff_pane(&bottom, 40, 6);
        assert!(rows[0].starts_with("diff · "), "the title stays on top");
        assert!(rows[5].starts_with("› M file19.rs"), "{rows:?}");

        let patch = WorkspaceDiffView {
            state: "patch".to_string(),
            vcs: "git".to_string(),
            path: "src/app.rs".to_string(),
            patch: Some(
                (0..30)
                    .map(|line| format!("+line{line:02}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            ..WorkspaceDiffView::default()
        };
        let pane = DiffPane {
            view: patch,
            selection: 0,
            scroll: 0,
        };
        let rows = diff_pane(&pane, 40, 6);
        assert!(rows[1].starts_with("+line00"), "{rows:?}");
        let scrolled = DiffPane { scroll: 10, ..pane };
        let rows = diff_pane(&scrolled, 40, 6);
        assert!(rows[1].starts_with("+line10"), "{rows:?}");
    }

    /// The listing window follows the cursor and the document window follows its
    /// own offset, and the title stays on row one of either: a reader who scrolled
    /// still knows what they are reading and how to get back.
    #[test]
    fn the_files_pane_windows_its_rows_under_a_stable_title() {
        let entry = |index: usize| WorkspaceEntryView {
            name: format!("file{index:02}.rs"),
            dir: false,
            size: index as u64,
        };
        let listing = WorkspaceReport {
            root: "/w".to_string(),
            path: String::new(),
            state: "dir".to_string(),
            entries: (0..20).map(entry).collect(),
            ..WorkspaceReport::default()
        };
        let pane = FilesPane {
            report: listing,
            selection: 0,
            scroll: 0,
        };
        let rows = files_pane(&pane, 40, 6);
        assert_eq!(rows.len(), 6, "the pane fills its body exactly");
        assert!(
            rows[0].contains("files · /"),
            "the root is named as one: {rows:?}"
        );
        assert!(rows[1].starts_with("› file00.rs"), "{rows:?}");

        // Walking the cursor to the last row keeps it in the last visible slot
        // rather than scrolling past it, which is what a file manager does.
        let bottom = FilesPane {
            selection: 19,
            ..pane
        };
        let rows = files_pane(&bottom, 40, 6);
        assert!(rows[5].starts_with("› file19.rs"), "{rows:?}");
        assert!(rows[0].contains("files ·"), "the title stays on row one");

        // A document scrolls by its own offset and clamps at its end, beside the
        // title that still says how many lines it holds.
        let document = WorkspaceReport {
            path: "src/lib.rs".to_string(),
            state: "file".to_string(),
            content: Some(
                (0..12)
                    .map(|index| format!("// row {index}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            ..WorkspaceReport::default()
        };
        let pane = FilesPane {
            report: document,
            selection: 0,
            scroll: usize::MAX,
        };
        let rows = files_pane(&pane, 40, 6);
        assert!(rows[0].contains("12 line(s)"), "{rows:?}");
        assert!(
            rows.iter().any(|row| row == "// row 11"),
            "the window stops on the last line: {rows:?}"
        );
    }

    /// The pane keeps its title while the body scrolls, and fills the rows it
    /// was given so the composer below never jumps.
    #[test]
    fn the_memory_pane_windows_its_rows_beneath_a_stable_title() {
        let now = Utc::now();
        let record = |index: usize| MemoryView {
            id: format!("m{index}"),
            content: format!("record {index}"),
            kind: "note".to_string(),
            scope: "workspace".to_string(),
            scope_id: "bos".to_string(),
            lifecycle: "active".to_string(),
            source: "agent".to_string(),
            revision: 1,
            updated_at: now,
            score: None,
            reasons: String::new(),
        };
        let pane = MemoryPane {
            report: MemoryReport {
                memories: (0..12).map(record).collect(),
                state: "ready".to_string(),
                error: None,
            },
            query: None,
            scroll: 0,
        };
        let rows = memory_pane(&pane, 60, 6);
        assert_eq!(rows.len(), 6, "the pane fills the body exactly");
        assert!(rows[0].contains("memory · 12 record(s)"));

        // Scrolled past the end, the window stops on the last body row instead
        // of opening on a blank frame - and the title stays on top, so the
        // reader still knows what they are looking at.
        let bottom = MemoryPane {
            scroll: usize::MAX,
            ..pane
        };
        let rows = memory_pane(&bottom, 60, 6);
        assert_eq!(rows.len(), 6);
        assert!(
            rows[0].contains("12 record(s)"),
            "the title stays on the first row"
        );
        assert!(
            rows.iter().any(|row| row.contains("record 11")),
            "the newest record is in the last window: {rows:?}"
        );
    }

    /// One task row, as the Host's board carries it.
    fn task_row(title: &str, status: &str, section_fire: Option<u64>) -> TaskRowView {
        TaskRowView {
            id: format!("task-{}", title.replace(' ', "-")),
            title: title.to_string(),
            tier: "turn".to_string(),
            status: status.to_string(),
            host: "local".to_string(),
            session: "session-1".to_string(),
            parent: String::new(),
            deps: Vec::new(),
            created_ms: 1_700_000_000_000,
            next_fire_ms: section_fire,
            attempts: 1,
            verification: "not_requested".to_string(),
        }
    }

    /// One board: running, queued, scheduled, history, in the desktop's order.
    fn task_board(sections: Vec<(&str, Vec<TaskRowView>)>) -> TaskBoardReport {
        assert_eq!(sections.len(), 4, "the board always carries four sections");
        TaskBoardReport {
            root: "/Users/me/bos".to_string(),
            state: "ready".to_string(),
            sections: sections
                .into_iter()
                .map(|(section, rows)| TaskSectionView {
                    section: section.to_string(),
                    rows,
                })
                .collect(),
            error: None,
        }
    }

    /// One task's full record, as the Host would answer for a board row.
    fn task_detail(id: &str, attempts: Vec<TaskAttemptView>) -> TaskDetailReport {
        TaskDetailReport {
            root: "/Users/me/bos".to_string(),
            state: "ready".to_string(),
            id: id.to_string(),
            title: "ship the release".to_string(),
            intent: "cut the release branch, run the suite and publish".to_string(),
            tier: "turn".to_string(),
            status: "running".to_string(),
            host: "local".to_string(),
            parent: String::new(),
            target: String::new(),
            command: "cargo test --all".to_string(),
            trigger: "every 600s".to_string(),
            next_fire_ms: Some(1_700_000_600_000),
            created_ms: 1_700_000_000_000,
            updated_ms: 1_700_000_060_000,
            deps: vec!["build".to_string(), "lint".to_string()],
            max_steps: 20,
            max_tokens: 200_000,
            deadline_ms: 600_000,
            retry: "manual".to_string(),
            max_attempts: 3,
            verification: "pending".to_string(),
            verification_contract: "review".to_string(),
            attempts,
            error: None,
        }
    }

    /// One attempt of the open task.
    fn task_attempt(
        id: &str,
        status: &str,
        result: Option<&str>,
        error: Option<&str>,
        change_set: Option<&str>,
    ) -> TaskAttemptView {
        TaskAttemptView {
            id: id.to_string(),
            tier: "turn".to_string(),
            status: status.to_string(),
            run: String::new(),
            started_ms: Some(1_700_000_060_000),
            ended_ms: None,
            result: result.unwrap_or_default().to_string(),
            error: error.unwrap_or_default().to_string(),
            change_set: change_set.unwrap_or_default().to_string(),
            verification: "not_requested".to_string(),
        }
    }

    /// The record behind a board row reads as the desktop's expanded card: the
    /// intent first, then the facts a row could not carry, then every attempt
    /// with what it returned. A reader who typed the row's id asked for more
    /// than the summary, so every block the card draws has to be here.
    #[test]
    fn the_task_detail_draws_the_record_the_card_expands() {
        let now = DateTime::from_timestamp_millis(1_700_000_060_000).expect("a moment");
        let detail = task_detail(
            "task-ship-the-release",
            vec![
                task_attempt(
                    "task-ship-the-release-1",
                    "succeeded",
                    Some("pushed   the\ntag"),
                    None,
                    Some("cs-17"),
                ),
                task_attempt(
                    "task-ship-the-release-2",
                    "failed",
                    None,
                    Some("the suite timed out"),
                    None,
                ),
            ],
        );
        let body = task_detail_body(&detail, 120, now);
        let text = body.join("\n");
        assert!(body[0] == "intent", "the objective comes first: {body:?}");
        assert!(text.contains("cut the release branch"), "{text}");
        assert!(text.contains("depends on · 2 task(s)"), "{text}");
        assert!(text.contains("build, lint"), "{text}");
        assert!(text.contains("cargo test --all"), "{text}");
        assert!(
            text.contains(
                "20 step(s) · 200000 token(s) · 600s deadline · retry manual up to 3 attempt(s)"
            ),
            "the caps a row cannot carry: {text}"
        );
        assert!(text.contains("pending · contract review"), "{text}");
        assert!(text.contains("attempts · 2"), "{text}");
        assert!(
            text.contains("pushed the tag") && !text.contains("pushed   the"),
            "a result with a newline stays one row: {text}"
        );
        assert!(text.contains("review cs-17"), "{text}");
        assert!(
            text.contains("failed: the suite timed out"),
            "an attempt that failed says why: {text}"
        );
        assert!(
            text.contains("every 600s"),
            "the trigger names its cadence: {text}"
        );
        assert!(
            text.contains("next in 9m"),
            "the next fire is read from the record: {text}"
        );

        let title = task_detail_title(&detail);
        assert!(
            title.starts_with("task task-ship-the-release · ship the release"),
            "{title}"
        );
        assert!(title.contains("running"), "{title}");
        assert!(title.contains("2 attempt(s)"), "{title}");
        assert!(title.contains("Esc closes"), "{title}");
        let summary = task_detail_summary(&detail);
        assert!(
            summary.starts_with("task task-ship-the-release"),
            "{summary}"
        );
        assert!(summary.contains("2 attempt(s)"), "{summary}");
    }

    /// An id no record carries is the Host's answer, not the client's: the
    /// pane prints the reason the store gave and the title names the state, so a
    /// mistyped id reads as an unknown task instead of an empty one.
    #[test]
    fn an_unknown_task_shows_the_hosts_reason() {
        let now = DateTime::from_timestamp_millis(1_700_000_060_000).expect("a moment");
        let missing = TaskDetailReport {
            root: "/Users/me/bos".to_string(),
            state: "unknown".to_string(),
            id: "ghost".to_string(),
            error: Some("no task in the store carries the id `ghost`".to_string()),
            ..TaskDetailReport::default()
        };
        let body = task_detail_body(&missing, 200, now);
        assert!(
            body.join(" ")
                .contains("no task in the store carries the id `ghost`"),
            "{body:?}"
        );
        let title = task_detail_title(&missing);
        assert_eq!(title, "task ghost · unknown · Esc closes");
        let summary = task_detail_summary(&missing);
        assert!(summary.contains("unknown"), "{summary}");
        assert!(
            summary.contains("no task in the store carries the id `ghost`"),
            "the reason travels to the status line: {summary}"
        );
    }

    /// Opening a task keeps the pane's title on the first row and its body
    /// windowed beneath, the same shape the board pane has.
    #[test]
    fn an_open_task_windows_its_record_beneath_the_title() {
        let now = DateTime::from_timestamp_millis(1_700_000_060_000).expect("a moment");
        let pane = TasksPane {
            report: task_board(vec![
                ("running", Vec::new()),
                ("queued", Vec::new()),
                ("scheduled", Vec::new()),
                ("history", Vec::new()),
            ]),
            detail: Some(task_detail("task-ship-the-release", Vec::new())),
            graph: false,
            scroll: 0,
        };
        let lines = tasks_pane(&pane, 80, 8);
        assert_eq!(lines.len(), 8);
        assert!(
            lines[0].starts_with("task task-ship-the-release"),
            "{}",
            lines[0]
        );
        assert!(
            lines
                .iter()
                .any(|row| row.contains("cut the release branch")),
            "the record is the body: {lines:?}"
        );
        assert!(
            !lines.iter().any(|row| row.contains("RUNNING ·")),
            "the board is not drawn behind the open record: {lines:?}"
        );
        let _ = now;
    }

    /// `/tasks` parses without an argument as the board, and with one word as
    /// the id of a task to open. A second word is a usage line: an id is one
    /// token, so a line with two never named a record.
    #[test]
    fn the_tasks_command_lists_the_board_or_opens_one_task() {
        assert_eq!(
            SlashCommand::parse("/tasks"),
            Some(SlashCommand::Tasks(None))
        );
        assert_eq!(
            SlashCommand::parse("/tasks 8f3a-1c"),
            Some(SlashCommand::Tasks(Some("8f3a-1c".to_string())))
        );
        // `graph` is the desktop's graph tab, and it parses as one word like a
        // task id: the command decides which of the two the word is.
        assert_eq!(
            SlashCommand::parse("/tasks graph"),
            Some(SlashCommand::Tasks(Some("graph".to_string())))
        );
        assert_eq!(
            SlashCommand::parse("/tasks now extra"),
            Some(SlashCommand::Usage("usage: /tasks [id|graph]"))
        );
    }

    /// `/extensions` takes free text, because a package has an id and a
    /// display name and a reader may search either, or the tool it added.
    #[test]
    fn the_extensions_command_lists_or_searches() {
        assert_eq!(
            SlashCommand::parse("/extensions"),
            Some(SlashCommand::Extensions(None))
        );
        assert_eq!(
            SlashCommand::parse("/extensions docs"),
            Some(SlashCommand::Extensions(Some("docs".to_string())))
        );
        assert_eq!(
            SlashCommand::parse("/extensions  docs server "),
            Some(SlashCommand::Extensions(Some("docs server".to_string()))),
            "a search is the rest of the line, spaces included"
        );
    }

    /// One installed package for these tests, with the fields a row reads.
    fn extension(
        id: &str,
        version: &str,
        state: &str,
        health: &str,
        applied: bool,
        tools: &[&str],
        error: Option<&str>,
    ) -> ExtensionView {
        ExtensionView {
            id: id.to_string(),
            name: id.to_string(),
            version: version.to_string(),
            versions: vec![version.to_string()],
            runtime: "process".to_string(),
            trust: "workspace".to_string(),
            signature: "signed".to_string(),
            source: "market".to_string(),
            state: state.to_string(),
            health: health.to_string(),
            configured: true,
            applied,
            tools: tools.iter().map(|tool| tool.to_string()).collect(),
            commands: Vec::new(),
            panels: Vec::new(),
            error: error.map(str::to_string),
        }
    }

    fn extension_report(view: ExtensionView) -> ExtensionsReport {
        ExtensionsReport {
            state: "ready".to_string(),
            extensions: vec![view],
            error: None,
        }
    }

    /// A package row states what the desktop card states: the version, the
    /// state, the health and the names a turn can call; a package that failed
    /// says why, because its tools are listed and none of them is in force.
    #[test]
    fn an_extension_row_names_its_state_and_contributions() {
        let live = extension(
            "docs",
            "1.2.0",
            "active",
            "healthy",
            true,
            &["docs_read"],
            None,
        );
        let rows = extension_row(&live, 100);
        assert_eq!(rows[0], "• docs · 1.2.0 · active · healthy");
        assert_eq!(rows[1], "  process · workspace · signed · market");
        assert_eq!(rows[2], "  tools: docs_read");
        assert_eq!(rows.len(), 3, "a healthy row needs no reason: {rows:?}");

        let broken = extension(
            "search",
            "0.3.0",
            "failed",
            "unhealthy",
            false,
            &["search_query"],
            Some("the manifest is not valid"),
        );
        let rows = extension_row(&broken, 100);
        assert!(
            rows[0].contains("configured, not applied"),
            "a switched-on package that is not running says so: {}",
            rows[0]
        );
        assert!(
            rows.iter()
                .any(|row| row.contains("the manifest is not valid")),
            "the failure reason travels: {rows:?}"
        );

        let bare = extension("quiet", "1.0.0", "active", "healthy", true, &[], None);
        assert!(
            extension_row(&bare, 100)[2].contains("contributes nothing this Host can call"),
            "a package with no contributions says so"
        );
    }

    /// A roster the Host could not read is stated with its reason, and an empty
    /// or unmatched roster says which of the two it is.
    #[test]
    fn the_extensions_body_names_the_reason_it_has_no_rows() {
        let failed = ExtensionsReport {
            state: "failed".to_string(),
            extensions: Vec::new(),
            error: Some("the host refused to open".to_string()),
        };
        let body = extensions_body(&failed, None, 80);
        assert_eq!(body.len(), 1);
        assert!(body[0].contains("could not be read"), "{body:?}");
        assert!(body[0].contains("the host refused to open"), "{body:?}");

        let empty = ExtensionsReport {
            state: "ready".to_string(),
            ..ExtensionsReport::default()
        };
        assert!(
            extensions_body(&empty, None, 80)[0].contains("no extension is installed here"),
            "an empty roster says it is empty"
        );

        let docs = extension_report(extension(
            "docs",
            "1.0.0",
            "active",
            "healthy",
            true,
            &[],
            None,
        ));
        assert!(extensions_body(&docs, Some("widgets"), 80)[0].contains("no extension matched"));
        // The search reads a contributed tool name, which is the question a
        // reader has when they know the tool and not the package.
        let tool = extension_report(extension(
            "pkg",
            "1.0.0",
            "active",
            "healthy",
            true,
            &["pkg_search"],
            None,
        ));
        assert_eq!(extensions_body(&tool, Some("pkg_search"), 80).len(), 3);
    }

    /// The status line counts what is in force and what failed, so a reader who
    /// just typed `/extensions` sees the roster's health before the rows.
    #[test]
    fn the_extensions_summary_counts_applied_and_failed() {
        let mut report = ExtensionsReport {
            state: "ready".to_string(),
            extensions: vec![
                extension("docs", "1.0.0", "active", "healthy", true, &[], None),
                extension("quiet", "1.0.0", "installed", "healthy", false, &[], None),
                extension(
                    "broken",
                    "1.0.0",
                    "failed",
                    "unhealthy",
                    false,
                    &[],
                    Some("boom"),
                ),
            ],
            error: None,
        };
        let summary = extensions_summary(&report, None);
        assert!(summary.contains("3 package(s)"), "{summary}");
        assert!(summary.contains("1 applied"), "{summary}");
        assert!(summary.contains("1 failed"), "{summary}");
        assert!(
            summary.ends_with("Esc closes · Up/Down scrolls"),
            "{summary}"
        );

        report.extensions.clear();
        let empty = extensions_summary(&report, None);
        assert_eq!(
            empty,
            "extensions · no package is installed here · Esc closes · Up/Down scrolls"
        );
        let searched = extensions_summary(&report, Some("docs"));
        assert!(searched.contains("\"docs\""), "{searched}");

        let unread = extensions_summary(
            &ExtensionsReport {
                state: "unsupported".to_string(),
                error: Some("this Host names no workspace directory".to_string()),
                ..ExtensionsReport::default()
            },
            None,
        );
        assert!(unread.starts_with("extensions · unsupported"), "{unread}");
        assert!(unread.contains("no workspace directory"), "{unread}");
    }

    /// The pane keeps its title on the first row and windows the body beneath
    /// it, so a scrolled roster still says what it is and how to leave.
    #[test]
    fn the_extensions_pane_windows_its_rows_beneath_a_stable_title() {
        let mut report = ExtensionsReport {
            state: "ready".to_string(),
            ..ExtensionsReport::default()
        };
        for index in 0..6 {
            report.extensions.push(extension(
                &format!("pkg-{index}"),
                "1.0.0",
                "active",
                "healthy",
                true,
                &[],
                None,
            ));
        }
        let pane = ExtensionsPane {
            report: report.clone(),
            filter: None,
            scroll: 0,
        };
        let rows = extensions_pane(&pane, 70, 5);
        assert_eq!(rows.len(), 5, "the pane fills the rows it is given");
        assert!(
            rows[0].starts_with("extensions · 6 package(s)"),
            "{}",
            rows[0]
        );
        assert!(rows[1].starts_with("• pkg-0"), "{}", rows[1]);

        // A scroll past the end clamps to the last body page, and the title
        // stays on row one.
        let bottom = ExtensionsPane {
            report,
            filter: Some("pkg-5".to_string()),
            scroll: usize::MAX / 2,
        };
        let rows = extensions_pane(&bottom, 70, 4);
        assert!(
            rows[0].starts_with("extensions · \"pkg-5\" · 1 of 6 package(s)"),
            "{}",
            rows[0]
        );
        assert!(rows[1].starts_with("• pkg-5"), "{}", rows[1]);
    }

    /// The Review pane is the desktop Review tab in one column: the changeset's
    /// own facts, its change list under a highlight, and the findings the risk
    /// level was read from.
    #[test]
    fn the_review_pane_lists_the_changeset_the_desktop_tab_draws() {
        assert_eq!(
            SlashCommand::parse("/review"),
            Some(SlashCommand::Review(None))
        );
        assert_eq!(
            SlashCommand::parse("/review src/app.rs"),
            Some(SlashCommand::Review(Some("src/app.rs".to_string())))
        );
        // A change id is opaque: it is taken verbatim rather than split on the
        // dashes a uuid carries.
        assert_eq!(
            SlashCommand::parse("/review 8f3a-2c1b"),
            Some(SlashCommand::Review(Some("8f3a-2c1b".to_string())))
        );

        let change = |path: &str, kind: &str| ReviewChangeView {
            id: format!("change-{path}"),
            path: path.to_string(),
            kind: kind.to_string(),
            tools: vec!["write_file".to_string(), "edit_file".to_string()],
            revisions: 2,
            ..ReviewChangeView::default()
        };
        let report = ReviewReport {
            state: "ready".to_string(),
            session: "session-a".to_string(),
            change_set: Some("9f1c2d3e-4a5b-6c7d".to_string()),
            turn: Some("3".to_string()),
            capture: "exact".to_string(),
            changeset_status: "sealed".to_string(),
            digest: Some("a1b2c3d4e5f6".to_string()),
            status: "in_review".to_string(),
            risk: "medium".to_string(),
            requires_human: true,
            changes: vec![
                change("src/lib.rs", "modified"),
                change("src/app.rs", "created"),
            ],
            findings: vec![ReviewFindingView {
                severity: "medium".to_string(),
                kind: "test".to_string(),
                status: "open".to_string(),
                title: "a changed file has no test".to_string(),
                ..ReviewFindingView::default()
            }],
            ..ReviewReport::default()
        };

        // The status row counts the changes and says how the list is left.
        let summary = review_summary(&report, None);
        assert!(
            summary.starts_with("review · 2 change(s) · sealed"),
            "{summary}"
        );
        assert!(summary.contains("risk medium"), "{summary}");
        assert!(summary.contains("Esc closes"), "{summary}");

        let pane = ReviewPane {
            report: report.clone(),
            selection: 0,
            scroll: 0,
        };
        let rows = review_pane(&pane, 80, 12);
        assert!(rows[0].contains("2 change(s)"), "{}", rows[0]);
        assert!(rows[0].contains("needs a human"), "{}", rows[0]);
        // The changeset is named the way the desktop repeats its id, so a reader
        // can match the row against the desktop's tab.
        assert!(rows[1].contains("changeset 9f1c2d3e"), "{}", rows[1]);
        assert!(rows[1].contains("turn 3"), "{}", rows[1]);
        assert!(rows[2].contains("captured exact"), "{}", rows[2]);
        assert!(rows[2].contains("digest a1b2c3d4"), "{}", rows[2]);
        // The change rows are the desktop's: path, effect, revision count and
        // the tools, with the first tool counting the rest.
        let first = rows
            .iter()
            .find(|row| row.starts_with("› src/lib.rs"))
            .expect("the highlighted change");
        assert!(
            first.contains("· modified · 2 rev · write_file +1"),
            "{first}"
        );
        assert!(
            rows.iter().any(|row| row.starts_with("  src/app.rs")),
            "an unselected change keeps the same indent: {rows:?}"
        );
        assert!(
            rows.iter().any(|row| row.contains("findings · 1 row(s)")),
            "{rows:?}"
        );

        // Walking the list moves the highlight, and a short window follows it
        // rather than leaving the selected change off screen.
        let moved = ReviewPane {
            report: report.clone(),
            selection: 1,
            scroll: 0,
        };
        let rows = review_pane(&moved, 80, 5);
        assert!(
            rows.iter().any(|row| row.starts_with("› src/app.rs")),
            "{rows:?}"
        );

        // A report with no sealed changeset says which state it is in, and why.
        let none = ReviewReport {
            state: "none".to_string(),
            error: Some("the turn that changed these files has not sealed".to_string()),
            ..ReviewReport::default()
        };
        let summary = review_summary(&none, None);
        assert!(summary.contains("has not sealed"), "{summary}");
        let pane = ReviewPane {
            report: none,
            selection: 0,
            scroll: 0,
        };
        let rows = review_pane(&pane, 80, 6);
        assert!(rows[0].starts_with("review · none"), "{}", rows[0]);
        assert!(
            rows.iter().any(|row| row.contains("no changeset to read")),
            "{rows:?}"
        );
    }

    /// Naming a change replaces the list with its prepared diff, which is the
    /// row a click opens on the desktop: the same net effect, the same
    /// structural nodes and the same unified text rows.
    #[test]
    fn a_named_change_opens_its_prepared_diff() {
        let diff = ReviewDiffView {
            path: "src/lib.rs".to_string(),
            kind: "modified".to_string(),
            added: 1,
            removed: 1,
            old_bytes: 12,
            new_bytes: 12,
            structural: vec![ReviewNodeView {
                id: "node-1".to_string(),
                kind: "function".to_string(),
                name: "crate::old".to_string(),
                change: "removed".to_string(),
                old_line: Some(1),
                ..ReviewNodeView::default()
            }],
            lines: vec![
                ReviewDiffLineView {
                    mark: "-".to_string(),
                    old_line: Some(1),
                    new_line: None,
                    text: "fn old() {}".to_string(),
                },
                ReviewDiffLineView {
                    mark: "+".to_string(),
                    old_line: None,
                    new_line: Some(1),
                    text: "fn new() {}".to_string(),
                },
            ],
            note: Some("the change was captured exactly".to_string()),
            ..ReviewDiffView::default()
        };
        let report = ReviewReport {
            state: "ready".to_string(),
            session: "session-b".to_string(),
            change_set: Some("9f1c2d3e".to_string()),
            capture: "exact".to_string(),
            changeset_status: "sealed".to_string(),
            risk: "low".to_string(),
            diff: Some(diff),
            verifications: vec![ReviewVerificationView {
                kind: "test".to_string(),
                status: "passed".to_string(),
                command: Some("cargo test -p bsh".to_string()),
                exit_code: Some(0),
                summary: "41 passed".to_string(),
            }],
            decisions: vec![ReviewDecisionView {
                decision: "approve".to_string(),
                actor: "local-user".to_string(),
                digest: "a1b2c3d4e5f6".to_string(),
                at_ms: 1_700_000_000_000,
                ..ReviewDecisionView::default()
            }],
            ..ReviewReport::default()
        };
        let pane = ReviewPane {
            report,
            selection: 0,
            scroll: 0,
        };
        let rows = review_pane(&pane, 80, 12);
        // A diff offers the way back, not a walk of rows it does not have.
        assert!(rows[0].contains("src/lib.rs"), "{}", rows[0]);
        assert!(rows[0].contains("Esc goes back"), "{}", rows[0]);
        assert!(!rows[0].contains("Up/Down walks"), "{}", rows[0]);
        assert!(rows[1].contains("+1 -1"), "{}", rows[1]);
        assert!(rows[1].contains("12 → 12 bytes"), "{}", rows[1]);
        assert!(rows[2].contains("captured exactly"), "{}", rows[2]);
        assert!(
            rows.iter()
                .any(|row| row.contains("structural · 1 node(s)")),
            "{rows:?}"
        );
        assert!(
            rows.iter()
                .any(|row| row.contains("removed function · crate::old · was line 1")),
            "{rows:?}"
        );
        // A removed row is numbered by the line it left, an added row by the
        // line it took: the gutter follows the side the mark names.
        assert!(
            rows.iter().any(|row| row == "-    1 fn old() {}"),
            "{rows:?}"
        );
        assert!(
            rows.iter().any(|row| row == "+    1 fn new() {}"),
            "{rows:?}"
        );

        // A change the changeset does not hold is answered with the list rather
        // than an empty diff.
        let missing = ReviewReport {
            state: "ready".to_string(),
            error: Some(
                "no change is named 'nope.rs' in this changeset; /review lists 1 change(s)"
                    .to_string(),
            ),
            changes: vec![ReviewChangeView {
                id: "change-1".to_string(),
                path: "src/lib.rs".to_string(),
                kind: "modified".to_string(),
                revisions: 1,
                ..ReviewChangeView::default()
            }],
            ..ReviewReport::default()
        };
        let summary = review_summary(&missing, Some("nope.rs"));
        assert!(summary.contains("nope.rs"), "{summary}");
        assert!(summary.contains("1 change(s)"), "{summary}");

        // The summary names the diff a named change opened, so a reader who
        // asked for a file sees the same words the pane's title carries.
        let opened = ReviewReport {
            state: "ready".to_string(),
            diff: Some(ReviewDiffView {
                path: "src/lib.rs".to_string(),
                kind: "modified".to_string(),
                added: 2,
                removed: 3,
                ..ReviewDiffView::default()
            }),
            ..ReviewReport::default()
        };
        let summary = review_summary(&opened, Some("src/lib.rs"));
        assert!(summary.contains("+2 -3"), "{summary}");
        assert!(summary.contains("Esc goes back"), "{summary}");
    }

    /// The board's rows read the way the desktop card reads them: the label
    /// first, then the state, tier and attempts, and the age last so a column
    /// sorts itself by recency.
    #[test]
    fn a_task_row_names_its_state_and_age() {
        let now = DateTime::from_timestamp_millis(1_700_000_060_000).expect("a moment");
        let row = task_row("ship the release", "running", None);
        let line = tasks_row(&row, 200, now);
        assert!(line.starts_with("• ship the release · running"), "{line}");
        assert!(line.contains("· turn"), "{line}");
        assert!(
            !line.contains("local"),
            "a local task does not say so: {line}"
        );
        assert!(line.contains("1 attempt(s)"), "{line}");
        assert!(
            !line.contains("verified"),
            "an unrequested verification is not a claim: {line}"
        );
        assert!(line.ends_with(" · 1m"), "the age closes the row: {line}");

        // A peer-owned task says who runs it, and a requested verification says
        // how it went: both are facts a reader decides on.
        let peer = TaskRowView {
            host: "peer".to_string(),
            verification: "passed".to_string(),
            next_fire_ms: Some(1_700_000_180_000),
            ..task_row("mirror the peer board", "queued", None)
        };
        let line = tasks_row(&peer, 200, now);
        assert!(line.contains("· peer"), "{line}");
        assert!(line.contains("verified passed"), "{line}");
        assert!(line.contains("in 2m"), "the next fire is relative: {line}");
    }

    /// The body draws the four sections under their own headers, and an empty
    /// section is still an answer.
    #[test]
    fn the_task_board_body_draws_every_section() {
        let now = DateTime::from_timestamp_millis(1_700_000_060_000).expect("a moment");
        let board = task_board(vec![
            (
                "running",
                vec![task_row("ship the release", "running", None)],
            ),
            ("queued", Vec::new()),
            (
                "scheduled",
                vec![task_row(
                    "refresh the index",
                    "scheduled",
                    Some(1_700_000_120_000),
                )],
            ),
            ("history", Vec::new()),
        ]);
        let body = tasks_board_body(&board, 200, now);
        let headers: Vec<&String> = body.iter().filter(|row| row.contains("task(s)")).collect();
        assert_eq!(headers.len(), 4, "every section is drawn: {body:?}");
        assert!(body[0].starts_with("RUNNING · 1 task(s)"), "{}", body[0]);
        assert!(
            body.iter().any(|row| row.contains("ship the release")),
            "{body:?}"
        );
        assert!(
            body.iter().any(|row| row.contains("QUEUED · 0 task(s)")),
            "an empty section states its zero: {body:?}"
        );
        assert_eq!(tasks_count(&board), 2);
        assert_eq!(tasks_section_count(&board, "running"), 1);

        // An empty board says so once, and an unread store names its reason
        // instead of looking like an empty workspace.
        let empty = tasks_board_body(
            &task_board(vec![
                ("running", Vec::new()),
                ("queued", Vec::new()),
                ("scheduled", Vec::new()),
                ("history", Vec::new()),
            ]),
            60,
            now,
        );
        assert!(empty.join(" ").contains("holds no task yet"), "{empty:?}");
        let unsupported = tasks_board_body(
            &TaskBoardReport {
                state: "unsupported".to_string(),
                ..TaskBoardReport::default()
            },
            60,
            now,
        );
        assert!(
            unsupported.join(" ").contains("carries no task board"),
            "{unsupported:?}"
        );
    }

    /// The title and status rows state the board's numbers, and a Host with no
    /// task surface names the state rather than four zeroes.
    #[test]
    fn the_task_board_states_its_numbers() {
        let board = task_board(vec![
            (
                "running",
                vec![task_row("ship the release", "running", None)],
            ),
            ("queued", Vec::new()),
            ("scheduled", Vec::new()),
            ("history", vec![task_row("cut the tag", "succeeded", None)]),
        ]);
        let summary = tasks_summary(&board);
        assert!(summary.contains("2 task(s)"), "{summary}");
        assert!(summary.contains("1 running"), "{summary}");
        assert!(summary.contains("1 settled"), "{summary}");
        assert!(summary.contains("Up/Down scrolls"), "{summary}");
        let title = tasks_title(&TasksPane {
            report: board,
            detail: None,
            graph: false,
            scroll: 0,
        });
        assert!(
            title.starts_with("tasks · /Users/me/bos · 2 task(s)"),
            "{title}"
        );
        assert!(title.contains("Esc closes"), "{title}");

        let unsupported = TaskBoardReport {
            state: "unsupported".to_string(),
            error: Some("this Host names no workspace directory".to_string()),
            ..TaskBoardReport::default()
        };
        let summary = tasks_summary(&unsupported);
        assert!(summary.contains("unsupported"), "{summary}");
        assert!(
            summary.contains("names no workspace directory"),
            "the reason travels: {summary}"
        );
    }

    /// The pane fills its rows, keeps its title on the first one, and clamps a
    /// scroll past the end instead of opening on a blank frame.
    #[test]
    fn the_task_pane_windows_its_rows_beneath_a_stable_title() {
        let mut rows: Vec<TaskRowView> = (0..12)
            .map(|index| task_row(&format!("task {index}"), "running", None))
            .collect();
        rows.sort_by(|left, right| left.title.cmp(&right.title));
        let board = task_board(vec![
            ("running", rows),
            ("queued", Vec::new()),
            ("scheduled", Vec::new()),
            ("history", Vec::new()),
        ]);
        let pane = TasksPane {
            report: board,
            detail: None,
            graph: false,
            scroll: 0,
        };
        let lines = tasks_pane(&pane, 60, 6);
        assert_eq!(lines.len(), 6, "the pane fills the body exactly");
        assert!(lines[0].contains("tasks · /Users/me/bos"), "{}", lines[0]);
        assert!(lines[0].contains("12 task(s)"), "{}", lines[0]);

        let bottom = TasksPane {
            scroll: usize::MAX,
            ..pane
        };
        let lines = tasks_pane(&bottom, 60, 6);
        assert_eq!(lines.len(), 6);
        assert!(
            lines[0].contains("12 task(s)"),
            "the title stays on the first row"
        );
        assert!(
            lines.iter().any(|row| row.contains("HISTORY")),
            "the window is the tail of the board, not an empty frame: {lines:?}"
        );
    }

    /// `/tasks graph` draws the desktop's graph tab as a tree: a child hangs
    /// under the parent it names, a row that waits on another says so, and a
    /// parent the board does not hold is named rather than invented.
    #[test]
    fn the_task_graph_nests_children_and_names_their_edges() {
        let now = DateTime::from_timestamp_millis(1_700_000_060_000).expect("a moment");
        let plan = task_row("plan the release", "running", None);
        let branch = TaskRowView {
            parent: plan.id.clone(),
            deps: vec![plan.id.clone()],
            ..task_row("cut the release branch", "queued", None)
        };
        let tag = TaskRowView {
            parent: plan.id.clone(),
            deps: vec![branch.id.clone()],
            ..task_row("tag the release", "queued", None)
        };
        let orphan = TaskRowView {
            parent: "task-not-on-the-board".to_string(),
            ..task_row("mirror the peer board", "running", None)
        };
        let sweep = task_row("sweep the index", "running", None);
        let board = task_board(vec![
            ("running", vec![orphan, plan, sweep]),
            ("queued", vec![branch, tag]),
            ("scheduled", Vec::new()),
            ("history", Vec::new()),
        ]);
        let body = tasks_graph_body(&board, 120, now).join("\n");
        // The trunk states what the graph holds and how many edges it draws:
        // the two children of the plan, and the two dependencies.
        assert!(
            body.starts_with("▸ /Users/me/bos · 5 task(s) · 4 edge(s)"),
            "{body}"
        );
        assert!(
            body.contains("├─ • mirror the peer board"),
            "a row whose parent is off the board still hangs at the trunk: {body}"
        );
        assert!(body.contains("├─ • plan the release"), "{body}");
        assert!(body.contains("└─ • sweep the index"), "{body}");
        // The plan's children hang off the connector its own branch owns.
        assert!(body.contains("│  ├─ • cut the release branch"), "{body}");
        assert!(body.contains("│  └─ • tag the release"), "{body}");
        // A dependency is an edge the tree cannot draw, so it is spelled out.
        assert!(body.contains("· waits on task-plan-the-release"), "{body}");
        assert!(
            body.contains("· waits on task-cut-the-release-branch"),
            "{body}"
        );
        assert!(
            body.contains("· parent task-not-on-the-board is off this board"),
            "{body}"
        );
        assert_eq!(tasks_edge_count(&board), 4);

        let pane = TasksPane {
            report: board.clone(),
            detail: None,
            graph: true,
            scroll: 0,
        };
        let title = tasks_title(&pane);
        assert!(
            title.starts_with("tasks graph · /Users/me/bos · 5 task(s) · 4 edge(s)"),
            "{title}"
        );
        assert!(title.contains("/tasks lists the board"), "{title}");
        let lines = tasks_pane(&pane, 100, 6);
        assert!(lines[0].contains("tasks graph"), "{}", lines[0]);
        assert!(
            lines
                .iter()
                .any(|row| row.contains("cut the release branch")),
            "the graph is the pane's body: {lines:?}"
        );
        let summary = tasks_graph_summary(&board);
        assert!(summary.contains("5 task(s) · 4 edge(s)"), "{summary}");
        assert!(summary.contains("Up/Down scrolls"), "{summary}");

        // The board stays the default: `/tasks` alone still draws its sections.
        let pane = TasksPane {
            report: board,
            detail: None,
            graph: false,
            scroll: 0,
        };
        assert!(
            tasks_body(&pane, 80, now)
                .iter()
                .any(|row| row.starts_with("RUNNING")),
            "the board view is unchanged"
        );
        // A Host with no task surface says where the graph asked and why.
        let unsupported = TaskBoardReport {
            state: "unsupported".to_string(),
            error: Some("this Host names no workspace directory".to_string()),
            ..TaskBoardReport::default()
        };
        let summary = tasks_graph_summary(&unsupported);
        assert!(
            summary.starts_with("tasks graph · unsupported"),
            "{summary}"
        );
        assert!(
            summary.contains("names no workspace directory"),
            "{summary}"
        );
        assert!(
            tasks_graph_body(&unsupported, 80, now)
                .join(" ")
                .contains("names no workspace directory"),
            "the body names the reason the board view names"
        );
    }

    /// A pair of records naming each other as parent is a cycle, not a tree, and
    /// a deep chain folds under a counted row: a model-written `parent` may not
    /// cost the pane its frame.
    #[test]
    fn the_task_graph_draws_a_cycle_once_and_folds_a_deep_chain() {
        let now = DateTime::from_timestamp_millis(1_700_000_060_000).expect("a moment");
        let mut first = task_row("first", "queued", None);
        let mut second = task_row("second", "queued", None);
        first.parent = second.id.clone();
        second.parent = first.id.clone();
        let board = task_board(vec![
            ("running", Vec::new()),
            ("queued", vec![first, second]),
            ("scheduled", Vec::new()),
            ("history", Vec::new()),
        ]);
        let body = tasks_graph_body(&board, 80, now).join("\n");
        assert_eq!(body.matches("• first").count(), 1, "{body}");
        assert_eq!(body.matches("• second").count(), 1, "{body}");
        assert_eq!(tasks_edge_count(&board), 2);

        // One chain longer than the depth cap: the drawn prefix is bounded and
        // the rest travels as one counted row rather than vanishing.
        let chain: Vec<TaskRowView> = (0..20)
            .map(|index| {
                let mut row = task_row(&format!("link {index}"), "running", None);
                if index > 0 {
                    row.parent = format!("task-link-{}", index - 1);
                }
                row
            })
            .collect();
        let board = task_board(vec![
            ("running", chain),
            ("queued", Vec::new()),
            ("scheduled", Vec::new()),
            ("history", Vec::new()),
        ]);
        let body = tasks_graph_body(&board, 80, now).join("\n");
        assert!(body.contains("└─ • link 0"), "{body}");
        assert!(body.contains("link 11"), "the last drawn level: {body}");
        assert!(
            !body.contains("link 12"),
            "the cap keeps the next level off the frame: {body}"
        );
        let folded = GRAPH_DEPTH.min(20);
        assert!(
            body.contains(&format!("… {} task(s) below the depth cap", 20 - folded)),
            "the fold is counted, not silent: {body}"
        );
    }

    /// One peer as a Federation report carries it: an id, an optional note,
    /// and one grant per capability, scoped to the workspace the desktop shows.
    fn federation_peer(name: Option<&str>, state: &str, grants: &[&str]) -> FederationPeerView {
        FederationPeerView {
            node: format!("{}-node-identifier", name.unwrap_or("unnamed")),
            name: name.map(str::to_string),
            state: state.to_string(),
            grants: grants
                .iter()
                .map(|capability| FederationGrantView {
                    capability: (*capability).to_string(),
                    operations: vec!["execute".to_string()],
                    resources: Vec::new(),
                    workspaces: vec!["/Users/me/bos".to_string()],
                    expires_ms: None,
                })
                .collect(),
            enrolled_ms: 1_700_000_000_000,
            expires_ms: None,
        }
    }

    /// `/federation` parses as one bare roster: the peer a reader wants is
    /// chosen from the rows, so an argument is a usage line, not a filter.
    #[test]
    fn the_federation_command_is_one_bare_roster() {
        assert_eq!(
            SlashCommand::parse("/federation"),
            Some(SlashCommand::Federation)
        );
        assert_eq!(
            SlashCommand::parse("/federation alice"),
            Some(SlashCommand::Usage("usage: /federation"))
        );
    }

    /// The roster states the node, each peer's trust and one row per grant, and
    /// a store that could not be read names why instead of counting zeroes.
    #[test]
    fn the_federation_pane_lists_the_peers_and_their_grants() {
        let mut alice = federation_peer(Some("alice"), "trusted", &["shell", "memory"]);
        alice.expires_ms = Some(1_700_500_000_000);
        let report = FederationReport {
            state: "ready".to_string(),
            enabled: true,
            profile: "default".to_string(),
            node: "a-node-identifier".to_string(),
            name: "bos-node".to_string(),
            peers: vec![alice, federation_peer(None, "revoked", &[])],
            error: None,
        };
        let now = DateTime::from_timestamp_millis(1_700_100_000_000).expect("a fixed clock");
        let body = federation_body(&report, 80, now).join("\n");
        assert!(
            body.contains("NODE · bos-node · a-node-identifier · on"),
            "{body}"
        );
        assert!(body.contains("profile default"), "{body}");
        assert!(body.contains("2 peer(s) · 1 trusted"), "{body}");
        // A named peer shows the note the enrollment carried beside the id
        // prefix the desktop draws; an unnamed one keeps its full id.
        assert!(
            body.contains("alice · alice-node · trusted · 2 grant(s)"),
            "{body}"
        );
        assert!(
            body.contains("unnamed-node-identifier · revoked · 0 grant(s)"),
            "{body}"
        );
        assert!(
            body.contains("    · shell · ops execute · resources any · workspaces /Users/me/bos"),
            "{body}"
        );
        assert!(body.contains("ends in 4d"), "the expiry travels: {body}");

        let summary = federation_summary(&report);
        assert!(
            summary.contains("ready · 2 peer(s) · 1 trusted · 2 grant(s)"),
            "{summary}"
        );
        let title = federation_title(&FederationPane {
            report: report.clone(),
            scroll: 0,
        });
        assert!(
            title.starts_with("federation · bos-node · 2 peer(s) · 1 trusted · 2 grant(s)"),
            "{title}"
        );
        assert!(title.contains("Esc closes"), "{title}");

        // A switched-off node still lists the peers that trust it, and says so
        // above the roster rather than showing an empty frame.
        let off = federation_body(
            &FederationReport {
                state: "off".to_string(),
                enabled: false,
                ..report.clone()
            },
            80,
            now,
        )
        .join("\n");
        assert!(off.contains("federation is off"), "{off}");
        assert!(off.contains("alice · alice-node"), "{off}");

        // A store that could not be read names the reason the Host returned.
        let unreadable = federation_body(
            &FederationReport {
                state: "unreadable".to_string(),
                error: Some("the peer store is not readable".to_string()),
                ..FederationReport::default()
            },
            80,
            now,
        )
        .join("\n");
        assert!(
            unreadable.contains("the peer store is not readable"),
            "{unreadable}"
        );
        let unsupported = federation_summary(&FederationReport {
            state: "unsupported".to_string(),
            ..FederationReport::default()
        });
        assert!(unsupported.contains("unsupported"), "{unsupported}");
        assert!(unsupported.contains("Esc closes"), "{unsupported}");
    }

    /// The pane fills its rows, keeps its title on the first one, and clamps a
    /// scroll past the end instead of opening on a blank frame.
    #[test]
    fn the_federation_pane_windows_its_roster_beneath_a_stable_title() {
        let peers: Vec<FederationPeerView> = (0..12)
            .map(|index| federation_peer(Some(&format!("peer {index}")), "trusted", &["shell"]))
            .collect();
        let report = FederationReport {
            state: "ready".to_string(),
            enabled: true,
            profile: "default".to_string(),
            node: "a-node-identifier".to_string(),
            name: "bos-node".to_string(),
            peers,
            error: None,
        };
        let pane = FederationPane { report, scroll: 0 };
        let lines = federation_pane(&pane, 60, 6);
        assert_eq!(lines.len(), 6, "the pane fills the body exactly");
        assert!(lines[0].contains("federation · bos-node"), "{}", lines[0]);
        assert!(lines[0].contains("12 peer(s)"), "{}", lines[0]);

        let bottom = FederationPane {
            scroll: usize::MAX,
            ..pane
        };
        let lines = federation_pane(&bottom, 60, 6);
        assert_eq!(lines.len(), 6);
        assert!(
            lines[0].contains("12 peer(s)"),
            "the title stays on the first row"
        );
        assert!(
            lines.iter().any(|row| row.contains("peer 11")),
            "the window is the tail of the roster, not an empty frame: {lines:?}"
        );
    }

    /// One projected turn, with the span a log row measures.
    fn turn(
        status: ExecutionStatus,
        started_at: DateTime<Utc>,
        finished_at: Option<DateTime<Utc>>,
        detail: Option<&str>,
    ) -> ExecutionView {
        ExecutionView {
            execution_id: ExecutionId::new(),
            status,
            started_at,
            finished_at,
            detail: detail.map(str::to_string),
        }
    }

    /// One projected message placed precisely in time and in a turn.
    fn timed_message(
        role: MessageRole,
        text: &str,
        at: DateTime<Utc>,
        execution_id: Option<ExecutionId>,
    ) -> MessageView {
        MessageView {
            message_id: crate::runtime::ids::MessageId::new(),
            role,
            text: text.to_string(),
            at,
            execution_id,
            compaction: false,
            usage: None,
        }
    }

    #[test]
    fn the_trajectory_groups_turns_and_keeps_events_without_one() {
        // The log parses bare and refuses an argument like every other command
        // with no argument of its own.
        assert_eq!(
            SlashCommand::parse("/trajectory"),
            Some(SlashCommand::Trajectory)
        );
        assert!(matches!(
            SlashCommand::parse("/trajectory 2"),
            Some(SlashCommand::Usage(_))
        ));

        let start = DateTime::from_timestamp(1_700_000_000, 0).expect("a real instant");
        let session_id = SessionId::new();
        let first = turn(
            ExecutionStatus::Completed,
            start,
            Some(start + chrono::Duration::milliseconds(1_500)),
            None,
        );
        let second = turn(
            ExecutionStatus::Failed,
            start + chrono::Duration::seconds(60),
            Some(start + chrono::Duration::seconds(62)),
            Some("the provider refused the call"),
        );
        let mut call = call("read_file", ToolCallStatus::Completed, "{}", None);
        call.execution_id = Some(second.execution_id);
        call.step = 2;
        call.duration_ms = Some(12);
        call.started_at = start + chrono::Duration::seconds(61);
        let mut session = projection(vec![call]);
        session.session_id = session_id;
        session.title = "Trajectory".to_string();
        session.executions = vec![second.clone(), first.clone()];
        session.messages = vec![
            timed_message(
                MessageRole::User,
                "read the file\nand a second line",
                start,
                Some(first.execution_id),
            ),
            timed_message(
                MessageRole::Assistant,
                "done",
                start + chrono::Duration::seconds(1),
                Some(first.execution_id),
            ),
            timed_message(
                MessageRole::User,
                "an approval nobody ran",
                start - chrono::Duration::seconds(5),
                None,
            ),
        ];

        let body =
            trajectory_body(Some(&session), 200, start + chrono::Duration::seconds(63)).join("\n");
        // The turns are ordered by when they started, not by the order the
        // projection happened to list them in. The clock is read in the
        // reader's zone, so the test names the turns rather than their times.
        assert!(
            body.find("▸ turn 1") < body.find("▸ turn 2"),
            "the turns are chronological: {body}"
        );
        assert!(body.contains("· 1.5s · done"), "{body}");
        assert!(
            body.contains("· 2.0s · failed · the provider refused the call"),
            "{body}"
        );
        // A message is its first line only: the transcript carries the rest.
        assert!(body.contains("you · read the file"), "{body}");
        assert!(!body.contains("and a second line"), "{body}");
        assert!(body.contains("bsh · done"), "{body}");
        assert!(body.contains("#2 read_file · done · 12ms"), "{body}");
        // The message with no execution keeps its own section instead of
        // disappearing from the record.
        assert!(body.contains("▸ no turn · 1 event(s)"), "{body}");
        assert!(body.contains("an approval nobody ran"), "{body}");

        let summary = trajectory_summary(Some(&session));
        assert!(summary.contains("2 turn(s)"), "{summary}");
        assert!(summary.contains("4 event(s)"), "{summary}");
        assert!(summary.contains("Esc closes"), "{summary}");
        assert!(trajectory_summary(None).contains("no session open"));
        assert!(
            trajectory_body(None, 80, start)
                .join("\n")
                .contains("no session open")
        );
    }

    /// A duration keeps the precision its row needs, and a running turn is
    /// measured to now rather than given a finish it has not reported.
    #[test]
    fn a_turn_reads_its_span_at_a_useful_precision() {
        assert_eq!(trajectory_duration(0), "0ms");
        assert_eq!(trajectory_duration(12), "12ms");
        assert_eq!(trajectory_duration(999), "999ms");
        assert_eq!(trajectory_duration(1_000), "1.0s");
        assert_eq!(trajectory_duration(1_949), "1.9s");
        assert_eq!(trajectory_duration(59_800), "59.8s");
        assert_eq!(trajectory_duration(60_000), "1m 00s");
        assert_eq!(trajectory_duration(3_600_000), "1h 00m");
        let start = Utc::now() - chrono::Duration::seconds(8);
        let running = turn(ExecutionStatus::Running, start, None, None);
        let row = trajectory_turn_row(3, &running, Utc::now());
        assert!(row.starts_with("▸ turn 3 · "), "{row}");
        assert!(row.contains("· running"), "{row}");
        assert!(
            row.contains("8.") && row.contains('s'),
            "a running turn is measured to now: {row}"
        );
        assert_eq!(execution_word(ExecutionStatus::WaitingApproval), "waiting");
        // A clock is a wall clock, not a date: three fields, same zone as the reader.
        assert_eq!(trajectory_clock(running.started_at).len(), 8);
    }

    /// The pane follows the newest row until the reader scrolls back, and a
    /// scroll past the top lands on the first row instead of a blank frame.
    #[test]
    fn the_trajectory_pane_follows_the_tail_and_clamps_its_scroll() {
        let start = Utc::now();
        let execution = turn(ExecutionStatus::Completed, start, Some(start), None);
        let mut session = projection(Vec::new());
        session.title = "Tail".to_string();
        session.executions = vec![execution.clone()];
        session.messages = (0..12)
            .map(|index| {
                timed_message(
                    MessageRole::Assistant,
                    &format!("row {index}"),
                    start + chrono::Duration::milliseconds(index as i64),
                    Some(execution.execution_id),
                )
            })
            .collect();

        let following = TrajectoryPane { back: 0 };
        let title = trajectory_title(&following, Some(&session));
        assert!(title.contains("trajectory · Tail · 1 turn(s)"), "{title}");
        assert!(title.contains("following the newest row"), "{title}");
        let lines = trajectory_pane(&following, Some(&session), 60, 6);
        assert_eq!(lines.len(), 6, "the pane fills the body exactly");
        assert!(lines[0].starts_with("trajectory · Tail"), "{}", lines[0]);
        assert!(
            lines.iter().any(|row| row.contains("row 11")),
            "the window is the tail of the log: {lines:?}"
        );

        let back = TrajectoryPane { back: 100 };
        let title = trajectory_title(&back, Some(&session));
        assert!(
            title.contains("scrolled back"),
            "the title says the window moved: {title}"
        );
        let lines = trajectory_pane(&back, Some(&session), 60, 6);
        assert!(
            lines.iter().any(|row| row.contains("turn 1")),
            "a scroll past the top lands on the first row: {lines:?}"
        );
    }

    /// A catalogue with one model per provider and one preset, so a list has
    /// two rows that differ in both columns.
    fn picker_catalogue() -> CatalogueView {
        CatalogueView {
            providers: vec![
                crate::runtime::contract::ProviderModels {
                    provider: "deepseek".to_string(),
                    models: vec!["deepseek-flash".to_string()],
                },
                crate::runtime::contract::ProviderModels {
                    provider: "openai".to_string(),
                    models: vec!["qwen3.8-flash".to_string()],
                },
            ],
            presets: vec!["review".to_string()],
            ..CatalogueView::default()
        }
    }

    fn list_of(kind: picker::Kind) -> picker::Picker {
        picker::Picker::new(kind.title(), kind.items(&picker_catalogue()))
    }

    #[test]
    fn a_bare_command_draws_its_choices_as_a_list_not_a_line_of_names() {
        let rows = picker_lines(&list_of(picker::Kind::Model), 48);
        // The first row says what is being chosen and that a keystroke narrows
        // it; without that, a list looks like transcript.
        assert!(rows[0].starts_with("choose a model"), "{}", rows[0]);
        assert!(rows[0].contains("type to narrow"), "{}", rows[0]);
        // One row per choice: the highlight on the first, the fact on the right
        // edge and on the same column for both rows, so the column reads down.
        assert!(rows[1].starts_with("› "), "{}", rows[1]);
        assert!(rows[1].trim_end().ends_with("deepseek"), "{}", rows[1]);
        assert!(rows[2].starts_with("  "), "{}", rows[2]);
        assert!(rows[2].trim_end().ends_with("openai"), "{}", rows[2]);
        assert_eq!(
            cells(&rows[1]),
            cells(&rows[2]),
            "the rows end on one column: {rows:?}"
        );
        assert_eq!(
            cells(&rows[1]),
            48,
            "the fact reaches the right edge: {rows:?}"
        );
        assert!(
            rows.iter().all(|row| cells(row) <= 48),
            "no row overflows the frame: {rows:?}"
        );
        // The last row says how much the list holds and which keys spend it.
        let footer = rows.last().expect("a footer row");
        assert!(footer.contains("2 choice(s)"), "{footer}");
        assert!(footer.contains("Enter picks"), "{footer}");
    }

    #[test]
    fn a_typed_filter_narrows_the_list_and_says_how_much_of_it_still_matches() {
        let mut list = list_of(picker::Kind::Model);
        for character in "qwen".chars() {
            list.typed(character);
        }
        let rows = picker_lines(&list, 48);
        assert!(rows[0].contains("filter 'qwen'"), "{}", rows[0]);
        assert_eq!(rows.len(), 3, "the title, the one match, the count");
        assert!(rows[1].contains("qwen3.8-flash"), "{}", rows[1]);
        assert!(
            rows[2].contains("1 of 2 match"),
            "a narrowed list says what it held: {}",
            rows[2]
        );
        // A filter that matches nothing still says what happened and how to get
        // out instead of drawing an empty box that reads as a closed list.
        for character in "zzz".chars() {
            list.typed(character);
        }
        let rows = picker_lines(&list, 48);
        assert!(rows[1].contains("none match"), "{}", rows[1]);
        assert!(rows[2].contains("Esc closes"), "{}", rows[2]);
    }

    #[test]
    fn a_long_list_draws_a_window_that_follows_the_highlight() {
        let items: Vec<picker::Item> = (0..12)
            .map(|index| picker::Item {
                label: format!("model-{index}"),
                detail: "openai".to_string(),
                choice: picker::Choice::Preset(format!("model-{index}")),
                active: false,
            })
            .collect();
        let mut list = picker::Picker::new("choose a model", items);
        let rows = picker_lines(&list, 48);
        assert_eq!(rows.len(), 10, "the title, eight rows, the count");
        assert!(list_of(picker::Kind::Model).len() < 12);
        assert!(
            rows.last().expect("a footer").starts_with("… "),
            "a window with more below says so: {}",
            rows.last().expect("a footer")
        );
        // Walk to the end: the window moves so the highlight stays drawn, and
        // nothing follows it any more.
        while list.move_by(1) {}
        let rows = picker_lines(&list, 48);
        assert!(rows[1].contains("model-4"), "the window moved: {rows:?}");
        assert!(
            rows[8].starts_with("› "),
            "the last choice is drawn: {rows:?}"
        );
        assert!(
            !rows.last().expect("a footer").starts_with("… "),
            "nothing follows the last row: {rows:?}"
        );
    }

    #[test]
    fn a_picked_row_commits_the_command_the_reader_could_have_typed() {
        // A pick and a typed name are the same gesture in the end: both land on
        // one `SlashCommand`, so a chosen row cannot take a different path to
        // the Host than the name it stands for.
        let models = list_of(picker::Kind::Model);
        assert_eq!(
            pick_command(models.choice().expect("the first row is highlighted")),
            SlashCommand::parse("/model deepseek deepseek-flash").expect("the typed form")
        );
        let presets = list_of(picker::Kind::Preset);
        assert_eq!(
            pick_command(presets.choice().expect("the first row is highlighted")),
            SlashCommand::parse("/preset review").expect("the typed form")
        );
        let modes = list_of(picker::Kind::Permission);
        assert_eq!(
            pick_command(modes.choice().expect("the first row is highlighted")),
            SlashCommand::parse("/permission read-only").expect("the typed form")
        );
    }

    /// A bare `/sessions` opens the workspace as a list, the way the desktop's
    /// session menu does; its rows are the printed listing's own numbers.
    #[test]
    fn a_bare_sessions_command_lists_the_workspace_as_rows_to_pick_from() {
        let now = Utc::now();
        let row = |title: &str, minutes: i64, messages: usize, pinned: bool| SessionSummary {
            session_id: SessionId::new(),
            title: title.to_string(),
            version: 1,
            updated_at: now - chrono::Duration::minutes(minutes),
            message_count: messages,
            preview: None,
            active_execution: None,
            pending_approvals: 0,
            pinned,
            archived: false,
        };
        let sessions = vec![
            row("draft the RFC", 2, 12, true),
            row("fix the build", 90, 3, false),
        ];
        let items = session_items(&sessions, Some(1), picker::Kind::Session);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "draft the RFC");
        assert!(items[0].detail.contains("2m"), "{}", items[0].detail);
        assert!(items[0].detail.contains("12 msg"), "{}", items[0].detail);
        assert!(items[0].detail.contains("pinned"), "{}", items[0].detail);
        // The open chat is the ticked row, so the list says where the reader is
        // as well as where they might go.
        assert!(!items[0].active);
        assert!(items[1].active);
        let mut list = picker::Picker::new(picker::Kind::Session.title(), items);
        let rows = picker_lines(&list, 60);
        assert!(rows[0].starts_with("open a session"), "{}", rows[0]);
        assert!(rows[1].starts_with("› "), "{}", rows[1]);
        // A list that carries a tick explains it; the sessions list does.
        assert!(
            picker_lines(&list, 200)
                .last()
                .is_some_and(|footer| footer.contains("marks the active row")),
            "the tick is named where a row shows it"
        );
        // Enter sends the number the printed listing prints, so a pick and a
        // typed number land on the same Host call.
        assert_eq!(
            pick_command(list.choice().expect("the first row is highlighted")),
            SlashCommand::Sessions(Some("1".to_string()))
        );
        // A filter narrows the list, and the number follows the row, not the
        // position it is drawn at.
        for character in "build".chars() {
            list.typed(character);
        }
        assert_eq!(list.rows().len(), 1);
        assert_eq!(
            pick_command(list.choice().expect("the one match")),
            SlashCommand::Sessions(Some("2".to_string()))
        );
    }

    #[test]
    fn the_bin_and_the_archive_open_as_lists_whose_pick_is_the_typed_command() {
        let now = Utc::now();
        let parked = |title: &str, minutes: i64| SessionSummary {
            session_id: SessionId::new(),
            title: title.to_string(),
            version: 1,
            updated_at: now - chrono::Duration::minutes(minutes),
            message_count: 4,
            preview: None,
            active_execution: None,
            pending_approvals: 0,
            pinned: false,
            archived: false,
        };

        // A bin row is the number `/restore` takes, and nothing in the bin is
        // the open chat, so no row carries a tick.
        let bin = session_items(
            &[parked("keep me", 5), parked("older", 90)],
            None,
            picker::Kind::Trash,
        );
        assert!(bin.iter().all(|item| !item.active));
        assert_eq!(bin[0].label, "keep me");
        let mut list = picker::Picker::new(picker::Kind::Trash.title(), bin);
        assert!(
            picker_lines(&list, 60)[0].starts_with("restore a parked session"),
            "the list says what Enter does"
        );
        // No row of the bin is ticked, so the footer does not explain a mark
        // the reader will never see.
        let footer = picker_lines(&list, 200).pop().expect("a footer row");
        assert!(
            !footer.contains("marks the active row"),
            "the bin has nothing to mark: {footer}"
        );
        assert!(footer.contains("Enter picks"), "{footer}");
        assert_eq!(
            pick_command(list.choice().expect("the first row is highlighted")),
            SlashCommand::Restore(Some("1".to_string()))
        );
        // A filter narrows what is drawn; the number stays the row's own, so
        // the pick still names the chat the printed listing numbered.
        for character in "older".chars() {
            list.typed(character);
        }
        assert_eq!(list.rows().len(), 1);
        assert_eq!(
            pick_command(list.choice().expect("the one match")),
            SlashCommand::Restore(Some("2".to_string()))
        );

        // The archive's pick is the Unarchive row of the desktop's session
        // menu, spelled as the command the same number is typed with.
        let archived = session_items(&[parked("hidden", 3)], None, picker::Kind::Archived);
        let list = picker::Picker::new(picker::Kind::Archived.title(), archived);
        assert_eq!(
            pick_command(list.choice().expect("the first row is highlighted")),
            SlashCommand::Sessions(Some("unarchive 1".to_string()))
        );

        // An empty list has no rows to walk, so each says why instead.
        assert!(picker::Kind::Trash.empty().contains("the bin is empty"));
        assert!(
            picker::Kind::Archived
                .empty()
                .contains("no archived sessions")
        );
        assert!(
            picker::Kind::Trash
                .items(&CatalogueView::default())
                .is_empty()
        );
        assert!(
            picker::Kind::Archived
                .items(&CatalogueView::default())
                .is_empty()
        );
    }

    #[test]
    fn a_click_on_a_drawn_choice_is_the_row_it_stands_in() {
        // The list's title is at `top`, so its first choice is one row below:
        // the pointer names a slot, and only a slot inside the window that was
        // actually drawn names a row.
        assert_eq!(
            list_offset(10, 10, 1, 0, 3, 12),
            None,
            "the title is not a choice"
        );
        assert_eq!(list_offset(11, 10, 1, 0, 3, 12), Some(0));
        assert_eq!(list_offset(13, 10, 1, 0, 3, 12), Some(2));
        assert_eq!(
            list_offset(14, 10, 1, 0, 3, 12),
            None,
            "the key line under the window names no choice"
        );
        assert_eq!(list_offset(9, 10, 1, 0, 3, 12), None, "above the list");
        // The window follows the highlight, so a scrolled list's first drawn
        // row is the tenth choice, not the first one.
        assert_eq!(list_offset(11, 10, 1, 9, 3, 12), Some(9));
        // A filter can leave fewer rows than the window holds: the empty slots
        // under the last match are not choices, so a click there does nothing.
        assert_eq!(list_offset(13, 10, 1, 0, 3, 2), None);
        assert_eq!(list_offset(12, 10, 1, 0, 3, 2), Some(1));
    }

    #[test]
    fn a_pointer_pointing_at_a_row_walks_the_list_to_it() {
        let items = picker::permission_items();
        assert!(items.len() >= 3, "the contract names several modes");
        let mut list = picker::Picker::new(picker::Kind::Permission.title(), items);
        // Every row fits the window, so nothing has scrolled yet.
        assert_eq!(list.window(), (0, list.rows().len()));
        assert!(
            list.select(1),
            "pointing at another row moves the highlight"
        );
        assert_eq!(list.selected(), 1);
        assert!(
            !list.select(1),
            "pointing at the row already lit is no change"
        );
        // Past the end is the last row, not nothing: a pointer can only ever
        // land in the window that was drawn.
        assert!(list.select(99));
        assert_eq!(list.selected(), list.rows().len() - 1);
        // A list with no rows has nowhere to point.
        let mut empty = picker::Picker::new("nothing", Vec::new());
        assert_eq!(empty.window(), (0, 0));
        assert!(!empty.select(0));
    }

    #[test]
    fn a_narrow_frame_keeps_the_choice_and_drops_the_fact() {
        // The choice is why the list is open; the fact beside it is what a
        // narrow frame can spend on it, so the fact goes first and the row
        // still fits.
        let narrow = picker_row("deepseek-v4-pro", "deepseek", 12, "› ");
        assert!(cells(&narrow) <= 12, "{narrow:?}");
        assert!(narrow.contains("deepseek"), "{narrow:?}");
        let wide = picker_row("glm", "openai", 20, "› ");
        assert_eq!(cells(&wide), 20, "the fact reaches the right edge");
        assert!(wide.ends_with("openai"), "{wide:?}");
    }
}
