//! The root view: state ownership, layout, and the streaming pump.

use std::collections::BTreeSet;

use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::text::TextViewState;
use gpui_kit::*;

use super::paging::{PageFlow, PageKey};
use super::settings::{SettingsInputs, SkillFilter, TrustConfirm, VaultInputs};
use super::DivExt;
use crate::runner::AgentEvent;
use crate::settings::mcp::McpScope;
use crate::state::{AppState, Overlay};
use crate::store::{OlderPage, TracePage};

/// How close to the end still counts as following the tail, in pixels.
const TAIL_EPSILON: f32 = 24.0;

/// How long a message row shows the copy check before returning to the glyph.
///
/// Measured: the reference's `onCopy` sets `copied` for 1000ms
/// (`window.setTimeout(() => setCopied(false), 1e3)`).
const COPY_FLASH: std::time::Duration = std::time::Duration::from_millis(1_000);

/// How far a drag must travel before it counts as a selection rather than a click.
///
/// Below this the press was a click - on a control, on a link, on plain text -
/// and copying whatever selection happened to be left on screen would be a
/// surprise rather than a service.
pub(crate) const SELECTION_DRAG_PX: f32 = 3.0;

// Bodies parsed since the process started, for the cache test.
#[cfg(test)]
thread_local! {
    static BODIES_PARSED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many markdown bodies this thread has parsed.
#[cfg(test)]
pub(crate) fn bodies_parsed() -> usize {
    BODIES_PARSED.with(|parsed| parsed.get())
}

/// The parsed markdown bodies still in use: text, the frame that last drew it,
/// and the state that parsed it.
type BodyStates = std::collections::BTreeMap<(u8, u64), (String, u64, Entity<TextViewState>)>;

/// How many parsed markdown bodies the shell keeps, in age order.
///
/// One screen of rows plus a screen of history: enough that scrolling and
/// paging never reparse, small enough that a long chat cannot accumulate a
/// parsed copy of every message it ever showed.
const BODY_STATES_MAX: usize = 48;

/// Whether a pointer gesture travelled far enough to be a SELECTION drag.
///
/// Copy-on-selection copies on the release of a drag, and a click must not: the
/// transcript is full of controls, and re-copying a stale selection because the
/// reader clicked a tool row would be a surprise. Split out from the handler so
/// the threshold is testable without synthesising a text selection, which the
/// headless harness cannot produce.
pub(crate) fn selection_drag(dx: f32, dy: f32) -> bool {
    (dx * dx + dy * dy).sqrt() >= SELECTION_DRAG_PX
}

/// The window's only view.
/// The custom preset editor: one TOML buffer keyed by preset id.
pub(crate) struct PresetEditor {
    pub(crate) id: String,
    pub(crate) text: Entity<TextareaState>,
}

/// What became of one submission.
///
/// The attachments ride back on a refusal because only the caller knows where
/// they belong: the composer's chip row, or the queue row that must not lose
/// them.
enum Submit {
    /// Queued behind a running turn, or the turn opened.
    Accepted,
    /// Refused before a turn opened, with the attachments to put back.
    Refused(Vec<crate::state::Attachment>),
}

pub(crate) struct Shell {
    /// The model list the last successful fetch returned, if any.
    pub(crate) models_fetched: Option<Vec<String>>,
    /// Whether the last fetch failed, which the pane explains.
    pub(crate) models_fetch_failed: bool,
    /// Whether the last fetch was asked for with no base URL.
    pub(crate) models_fetch_needs_base_url: bool,
    /// Whether a fetch is still in flight, so the trigger names the ask
    /// rather than repeating the request it is already running.
    pub(crate) models_fetching: bool,
    /// The fetched model ids currently ticked in the picker.
    pub(crate) models_selected: std::collections::BTreeSet<String>,
    /// All shell state.
    pub(crate) state: AppState,
    /// The composer's text engine.
    pub(crate) composer: Entity<TextareaState>,
    /// The placeholder key the composer currently carries, so the refresh only
    /// runs when the phase (hero, conversation, plan mode) actually flips.
    placeholder_key: &'static str,
    /// The field width and language the placeholder was fitted to.
    ///
    /// Fitting shapes text, so it reruns only when the card or the locale
    /// changes - `set_placeholder` always notifies, and a per-frame call would
    /// put a repaint in every frame.
    placeholder_fit: Option<(u32, crate::i18n::Lang)>,
    /// Whether the to-do panel's list is expanded. The reference starts it
    /// collapsed and keeps the state per mounted panel, not per session.
    pub(crate) todo_open: bool,
    /// The queue row's inline editor. The reference edits one queued row at a
    /// time, so a single engine serves the whole list.
    pub(crate) queue_edit: Entity<TextareaState>,
    /// The queued row being edited, named by its identity; None when none is.
    pub(crate) queue_edit_for: Option<u64>,
    /// Whether the queue dock's list is expanded. The reference starts it
    /// collapsed and shows a lone waiting message regardless.
    pub(crate) queue_open: bool,
    /// The session-name search field.
    pub(crate) search: Entity<InputState>,
    /// The plugin-name filter on the plugin list pane.
    pub(crate) plugin_search: Entity<InputState>,
    /// The egress allowlist entry field in the policy pane.
    pub(crate) policy_entry: Entity<InputState>,
    /// The preset card whose config details are expanded, if any.
    pub(crate) preset_details: Option<String>,
    /// The custom preset being edited as TOML, if any.
    pub(crate) preset_editor: Option<PresetEditor>,
    /// The parse error from the last preset editor save.
    pub(crate) preset_editor_error: Option<String>,
    /// The Trajectory tab's own search field.
    pub(crate) traj_search: Entity<InputState>,
    /// Whether the timeline draws each block at its ACTUAL duration.
    ///
    /// The reference keeps this as an independent boolean, not as one value of
    /// a measure rotation: off draws every block at a fixed equal width (the
    /// `Duration` button reads `Use actual duration`), on draws it as wide as
    /// the work took (`Use equal-width operations`). Persisted like the other
    /// view preferences, because the reference stores it in localStorage.
    pub(crate) traj_actual_duration: bool,
    /// Whether the timeline places blocks at their REAL timestamps rather than
    /// in sequence. The reference ships this switch hidden; bsh keeps the same
    /// four projections the pair produces (sequence / time / duration / actual).
    pub(crate) traj_actual_time: bool,
    /// Whether every turn in the trajectory table is folded to its summary row.
    ///
    /// The reference's `Turns` toolbar action is a collapse-all switch, not a
    /// strip measure: it folds each turn to one line and its glyph flips from
    /// the collapse box to the expand box.
    pub(crate) traj_turns_collapsed: bool,
    /// Whether every assistant step in the trajectory table is folded.
    pub(crate) traj_calls_collapsed: bool,
    /// Whether the pointer is over a collapsed-turn summary row.
    ///
    /// The kit's hover is a listener rather than a style closure here, so the
    /// state is recorded and the fill applied on the next frame.
    pub(crate) traj_summary_hover: bool,
    /// The selected trajectory event (an index into the rendered log).
    pub(crate) traj_select: Option<usize>,
    /// The detail panel's tab: 0 summary, 1 preview, 2 raw, 3 source.
    pub(crate) traj_tab: usize,
    /// What the trajectory search field holds, kept beside the entity so the
    /// view can filter without reading it per row.
    pub(crate) traj_query: String,
    /// The trajectory lane track's width as of the last prepaint, in logical
    /// pixels.
    ///
    /// The blocks are placed by fractions of the shared time axis, but their
    /// two-pixel floor and the right-edge clamp need the track's real width:
    /// `1.0` of the axis has to land on the track's own right edge, and a
    /// floor at that edge must shift left rather than paint over it. The track
    /// measures itself once per layout pass (its first child is a full-size
    /// sentinel) and the next frame's render reads the width. `None` is the
    /// first frame after the strip mounts; a block then falls back to the
    /// relative box, which is still clipped by the track, and the measured
    /// box lands on the next frame.
    pub(crate) traj_lane_w: std::rc::Rc<std::cell::Cell<Option<f32>>>,
    /// The Audit tab's own search field.
    pub(crate) audit_search: Entity<InputState>,
    /// Which measure the audit toolbar shows: 0 duration, 1 turns, 2 calls.
    pub(crate) audit_mode: usize,
    /// What the audit search field holds, kept beside the entity so the view
    /// can filter without reading it per row.
    pub(crate) audit_query: String,
    /// The turns the reader has opened, by 1-based turn number.
    pub(crate) audit_turn_open: BTreeSet<usize>,
    /// The calls the reader has opened, by (turn, call) position.
    pub(crate) audit_open: BTreeSet<(usize, usize)>,
    /// The calls whose full stored text is showing.
    pub(crate) audit_full: BTreeSet<(usize, usize)>,
    /// The model-id filter on the fetched-model picker.
    pub(crate) picker_search: Entity<InputState>,
    /// The task-name filter on the Automation tasks pane.
    pub(crate) task_search: Entity<InputState>,
    /// The new-task name field.
    pub(crate) task_name: Entity<InputState>,
    /// The new-task instruction field.
    pub(crate) task_prompt: Entity<InputState>,
    /// The new-task repeat interval in minutes.
    pub(crate) task_every: Entity<InputState>,
    /// The status filter: 0 all, 1 enabled, 2 inactive.
    pub(crate) task_filter: usize,
    /// The task whose delete control is awaiting confirmation.
    pub(crate) task_delete: Option<String>,
    /// The permission mode awaiting the full-access risk gate, if any.
    pub(crate) permission_confirm: Option<String>,
    /// The installed package awaiting the trust confirmation, if any.
    pub(crate) trust_confirm: Option<TrustConfirm>,
    /// Why the last trust decision could not be saved, shown in that dialog.
    pub(crate) trust_error: Option<String>,
    /// Whether the risk gate's acknowledgement box is ticked. The gate's own
    /// confirm stays disabled until it is, like the reference's.
    pub(crate) permission_acknowledged: bool,
    /// Set when the Show coding view toggle could not be saved, so the row can
    /// report the reference's own `developerTools.error` beside it.
    pub(crate) developer_tools_failed: bool,
    /// Whether the Keyboard shortcuts disclosure is open. The reference opens a
    /// shortcut sheet here; this shell has no key bindings to edit, so the row
    /// stays a disclosure and says what it can honestly report.
    pub(crate) shortcuts_open: bool,
    /// Set when the header action could not reach the settings file. The
    /// reference prints its own inline `openDocument.error` beside the button.
    pub(crate) config_open_failed: bool,

    /// Whether the Models pane's add-provider card is open. The reference
    /// swaps its dashed Add button for the card, so the two never show at once.
    pub(crate) models_add_open: bool,
    /// Which route the open add card offers: a shipped provider from the
    /// catalogue, or a hand-typed endpoint. The reference's segmented control.
    pub(crate) models_add_route: ModelsAddRoute,
    /// The provider id the add card is typed for.
    pub(crate) add_route: Entity<InputState>,
    /// The display name the add card is typed for; blank follows the id.
    pub(crate) add_display: Entity<InputState>,
    /// The base URL the add card is typed for.
    pub(crate) add_base_url: Entity<InputState>,
    /// The vault entry the add card chose for the provider it will create.
    ///
    /// A pending choice, not a draft field: the provider does not exist until
    /// Create, at which point this id lands on it. Choosing an entry clears the
    /// plaintext field, so a created provider never carries both sources.
    pub(crate) add_key_vault: Option<String>,
    /// The model leaves the open provider editor or add card is editing, with
    /// one text engine per row so a keystroke repaints its own row.
    pub(crate) model_rows: Vec<ModelRowEditor>,
    /// The model id the open provider editor is adding.
    ///
    /// The editor's own field: a model row is added to the provider already
    /// open, and the add-provider card no longer asks for a model at all.
    pub(crate) new_model_id: Entity<InputState>,
    /// The display name the editor's new model row will take.
    pub(crate) new_model_name: Entity<InputState>,
    /// Whether the provider editor's Customized settings fold is open.
    pub(crate) models_advanced_open: bool,
    /// The third-party provider the add card picked from the catalogue.
    pub(crate) catalogue_choice: Option<String>,
    /// Whether the catalogue's provider select has its options open.
    ///
    /// The reference's chooser is a `select`: one closed box naming the
    /// provider id, and the list of the rest is drawn only while the reader
    /// has it open, so a click anywhere else closes it again.
    pub(crate) catalogue_open: bool,
    /// The transcript scroll position, so the turn rail can jump within it.
    pub(crate) transcript_scroll: ScrollHandle,
    /// The measured height of every transcript row, so offscreen rows cost a
    /// spacer instead of a full message element on every frame.
    pub(crate) transcript_layout: std::cell::RefCell<super::transcript::TranscriptLayout>,
    /// Parsed markdown bodies, one state entity per text, reused across frames.
    ///
    /// A `TextView` built from TEXT hands the state that text every frame, and
    /// the state decides whether to reparse by comparing the ALLOCATION it was
    /// given - a fresh clone is always "new", so every visible row re-parsed
    /// and re-laid-out its whole body on every frame. Holding the state here
    /// and handing it to `TextView::new` keeps an unchanged body parsed, and a
    /// streamed one takes the append path.
    ///
    /// Keyed by (kind, content hash) and capped by age: two bodies with the same
    /// text share one parse, and a body that scrolls away is evicted.
    pub(crate) body_states: std::cell::RefCell<BodyStates>,
    /// This frame's number, for the body-state cache's age.
    pub(crate) frame_no: u64,
    /// The transcript content height as of the last frame. Following the tail
    /// compares against it, so content that grew this frame still counts as
    /// "the reader was at the bottom".
    tail_max: Option<Pixels>,
    /// The session the tail state belongs to, so switching sessions starts over.
    tail_session: Option<String>,
    /// Whether the newest message should stay in view. A rail jump clears it,
    /// and content growing while the reader is at the end sets it again.
    pub(crate) following_tail: bool,
    /// A transcript child the audit's jump asked for, and the Chat frames left
    /// to wait before asking.
    ///
    /// The request cannot be made while the Audit tab is mounted: it lands on a
    /// scroller that is not in the tree and is dropped with the unmounted
    /// element. It waits for one mounted Chat frame so the rows have bounds,
    /// then the next frame's prepaint applies it.
    pub(crate) audit_jump_to: Option<(usize, u8)>,
    /// The reasoning disclosures the reader has opened, by message index. A
    /// session switch closes them all, the way the reference resets a turn's
    /// disclosure state.
    pub(crate) think_open: std::collections::HashSet<usize>,
    /// The compaction markers the reader has expanded, by record index.
    ///
    /// Scoped like the reasoning disclosures: a session switch closes them,
    /// because the indices name rows in the transcript that just changed.
    pub(crate) compaction_open: std::collections::HashSet<usize>,
    /// The offsets the eased scroll last settled on.
    ///
    /// The kit's scroller applies a wheel delta on its own, and it does so before
    /// this shell's listener sees the event - so the handler reads how far it
    /// jumped and takes that back, letting the easing own the whole motion.
    pub(crate) chat_last_offset: f32,
    pub(crate) traj_last_offset: f32,
    /// The chat transcript's eased scroll, and the trajectory tab's.
    pub(crate) chat_smooth: super::smooth::Smooth,
    pub(crate) traj_smooth: super::smooth::Smooth,
    /// The trajectory tab's scroll position, so its wheel can be eased and a test
    /// can read it.
    pub(crate) traj_scroll: ScrollHandle,
    /// The audit list's scroll position.
    pub(crate) audit_scroll: ScrollHandle,
    /// The sidebar tree's scroll position, so the wheel, the keys and a test can
    /// all address the same offset.
    pub(crate) side_scroll: ScrollHandle,
    /// The tree's focus. Arrow, Page and Home/End scroll it while it holds focus,
    /// which is how a focused list behaves everywhere else.
    pub(crate) side_focus: FocusHandle,
    /// The rail's scroll position, so the active mark can be kept in view.
    pub(crate) rail_scroll: ScrollHandle,
    /// The settings dialog's options box, so a write inside the pane can be
    /// shown not to move it and a test can read the same offset the reader sees.
    pub(crate) settings_scroll: ScrollHandle,
    /// The turn whose mark the pointer is over, which opens its preview card.
    pub(crate) rail_hover: Option<usize>,
    /// The message whose copy control last copied, for the check mark.
    ///
    /// The reference swaps the copy glyph for a check for one second and then
    /// back; this is that state, keyed by message index so the row that copied
    /// is the one that says so.
    pub(crate) copied_message: Option<usize>,
    /// Where the pointer went down inside the transcript, for copy-on-selection.
    ///
    /// A drag that ends more than SELECTION_DRAG_PX away takes the window's
    /// text selection to the clipboard; a plain click must not, or every click
    /// would re-copy whatever selection was left over.
    pub(crate) selection_start: Option<Point<Pixels>>,
    /// The changed-files cards whose fold the reader has opened, by message
    /// index. Scoped like the other disclosures: a session switch closes them,
    /// because the indices name rows in the transcript that just changed.
    pub(crate) changes_open: std::collections::HashSet<usize>,
    /// The changed-file row the pointer is over, with where and when it got
    /// there. The reference's card previews on pointer move after a 500ms
    /// delay, so the row that armed the timer and the moment it did are the
    /// two facts the timer needs.
    pub(crate) changes_hover: Option<ChangesHover>,
    /// The changed file whose preview has cleared that delay, the path it
    /// explains, and the rows the card draws for it.
    ///
    /// Held already RESOLVED, because resolving it costs a git subprocess: the
    /// rows are computed once when the delay passes and re-read every frame
    /// after that.
    pub(crate) changes_preview:
        Option<(usize, usize, String, Vec<crate::ui::diff_pane::PreviewLine>)>,
    /// One changed-file summary per message, keyed on the steps it was built
    /// from. Parsing every write's arguments on every frame is exactly the
    /// per-frame work the crate's own rule forbids.
    pub(crate) changes_cache: std::cell::RefCell<ChangesCache>,
    /// The conversation column's width as of the last frame, less the sidebar.
    /// The rail's container rule and its card breadth are measured against it.
    pub(crate) conversation_w: Pixels,
    /// The chat content width a reader dragged out, or none while the column
    /// follows the reference's clamp. Loaded from settings.global.general.content_width.
    pub(crate) chat_user_w: Option<f32>,
    /// The width drag in flight, if any.
    pub(crate) chat_drag: Option<ChatDrag>,
    /// The handle the pointer is over, which is the reference's :hover half
    /// of the bar's visibility rule.
    pub(crate) chat_hover: Option<HandleSide>,
    /// The sidebar width a reader dragged out, before it is committed.
    /// Loaded from settings.global.general.sidebar_width.
    pub(crate) sidebar_user_w: Option<f32>,
    /// The sidebar resize in flight, if any.
    pub(crate) sidebar_drag: Option<SideDrag>,
    /// The terminal's screen as of its last version, shared with the paint.
    pub(crate) terminal_snapshot: std::rc::Rc<crate::terminal::Snapshot>,
    /// The terminal screen version that copy was taken at.
    pub(crate) terminal_version: u64,
    /// The terminal pane's own focus, so typing reaches its PTY.
    pub(crate) terminal_focus: FocusHandle,
    /// The cell grid the pane's last prepaint measured, applied on the
    /// terminal clock so the PTY is resized outside the paint phase, and read
    /// by the pointer handlers to turn a position into a cell.
    pub(crate) terminal_frame:
        std::rc::Rc<std::cell::Cell<Option<super::right_panel::TerminalFrame>>>,

    /// The session being renamed, by id, with the field that renames it.
    pub(crate) renaming_session: Option<String>,
    /// The workspace row being retitled, and its own engine for the same reason
    /// the session rename keeps one.
    pub(crate) renaming_workspace: Option<String>,
    /// Whether the recycle bin is open, and what it listed when it opened.
    pub(crate) trash_open: bool,
    /// The installed-plugin disclosures that are open, by settings id.
    pub(crate) installed_open: std::collections::BTreeSet<String>,
    /// The resolved installed-package view, cached by (path, mtime).
    ///
    /// The list asks it for metadata every frame; the first ask parses the
    /// package and later frames compare mtimes, so a render never re-reads a
    /// manifest it already holds. Cleared on save and on install/uninstall.
    pub(crate) installed_cache: crate::installed::InstalledCache,
    /// Which tab of the Extensions pane is showing: 0 = Plugin list,
    /// 1 = MCP, 2 = Skills, 3 = Plugin Market. The management surfaces live IN
    /// that pane, beside the list.
    pub(crate) plugins_tab: usize,
    /// The local market's discovered rows, empty until the tab is first opened.
    ///
    /// Discovery walks the Codex plugin caches and the DSH profile manifests, so
    /// it runs from the tab click (and the refresh control), never from render.
    pub(crate) market: Vec<crate::market::MarketPlugin>,
    /// Whether a discovery pass has run, so a tab switch does not rescan.
    pub(crate) market_loaded: bool,
    /// Why some market sources could not be read, if any.
    pub(crate) market_error: Option<String>,
    /// The market's name filter.
    pub(crate) market_search: Entity<InputState>,
    /// What that field holds, kept beside the entity so the list can filter.
    pub(crate) market_query: String,
    /// How many filtered market rows are shown. The catalogs carry thousands of
    /// remote entries, so the pane renders a window and Show more grows it.
    pub(crate) market_limit: usize,
    /// The "add a remote index source" field.
    pub(crate) market_url: Entity<InputState>,
    /// How many remote index fetches are in flight. A count, not a flag:
    /// the built-in catalogs answer at different times, and the spinner must
    /// survive the first one finishing.
    pub(crate) market_fetching: usize,
    /// Why a remote index fetch failed, if one did.
    pub(crate) market_fetch_error: Option<String>,
    /// The remote plugin currently downloading, by package id.
    pub(crate) market_installing: Option<String>,
    /// Why the last remote install failed, if it did.
    pub(crate) market_install_error: Option<String>,
    /// Whether the market's source editor is expanded. Collapsed by default so
    /// the catalog gets the pane; the header still names the built-in count.
    pub(crate) market_sources_open: bool,
    /// Why the last immediate skills write failed, if it did.
    ///
    /// The Skills switches do not wait for the dialog's Save, so a refused
    /// write is the only place a reader can learn that the switch did not
    /// reach disk: it is drawn in the pane rather than swallowed.
    pub(crate) skills_save_error: Option<String>,
    /// The Skills pane's own sub-view: whether the browse-all list and the
    /// executor-dirs list are open. Both are disclosures over the card grid, so
    /// they are pane state rather than settings.
    pub(crate) skills_browse_open: bool,
    pub(crate) skills_dirs_open: bool,
    /// Which of All / Enabled / Disabled the browse-all list shows.
    ///
    /// View state, not a setting: it is reset when the Settings dialog opens and
    /// when the Skills tab is pressed, so it can never quietly hide a skill on
    /// the next visit. See `SkillFilter`.
    pub(crate) skill_filter: SkillFilter,
    /// The skill-name filter on the browse-all list.
    pub(crate) skill_search: Entity<InputState>,
    /// What that field holds, kept beside the entity so the list can filter
    /// without reading the engine per row.
    pub(crate) skill_query: String,
    /// The source being added, as a path and a label, while the add control is
    /// open. Both are kept so the refusal can name what was typed.
    pub(crate) skill_add_open: bool,
    pub(crate) skill_add_path: Entity<InputState>,
    pub(crate) skill_add_name: Entity<InputState>,
    /// The row a just-loaded page of older messages should leave at the top,
    /// applied after it is painted so the reader's place does not jump.
    pub(crate) older_anchor: Option<usize>,
    /// The chat page flow: one read in flight plus one page read ahead. The
    /// Parquet read runs on the background executor, never on this thread.
    pub(crate) older: PageFlow<OlderPage>,
    /// Trajectory rows paged out of the Parquet log, oldest page first, and the
    /// number of pages loaded so far.
    pub(crate) trace_rows: Vec<crate::history::ToolRow>,
    /// The session the paged trace belongs to.
    pub(crate) trace_session: Option<String>,
    pub(crate) trace_pages: usize,
    /// The trajectory page flow: the same one-in-flight/one-ahead rule as the
    /// chat transcript.
    pub(crate) trace: PageFlow<TracePage>,
    /// Whether the log has no page past the ones already loaded.
    ///
    /// The trace asks for a page whenever the reader's wheel reaches the top,
    /// and an exhausted log answers empty; without this latch a reader parked
    /// at the top re-opened the Parquet index once per frame forever. A turn
    /// ending is the only event that can grow the log, so that clears it.
    trace_done: bool,
    /// The histories this shell has already checked for one-time compaction.
    /// A history is opened once per selection, and the rewrite is asked for
    /// once, never from a frame.
    compaction_checked: std::collections::HashSet<String>,
    pub(crate) trash_entries: Vec<crate::trash::Entry>,
    pub(crate) workspace_rename: Entity<InputState>,
    /// The rename field's engine, built once so moving rows keeps focus.
    pub(crate) session_rename: Entity<InputState>,
    /// Whether the pointer is on the panel's seam, which reveals its bar.
    pub(crate) right_hover: bool,
    /// The right panel's own state.
    pub(crate) right: super::right_panel::RightPanel,
    /// The workspace browser's place in the tree.
    pub(crate) files: super::right_panel::FilesPane,
    /// The viewport width as of the last frame, which is what the reference's
    /// sidebar auto-collapse measures.
    pub(crate) viewport_w: f32,
    /// The background job whose stop button is armed (or showing a failure),
    /// with the instant that phase began.
    pub(crate) jobs_arm: Option<super::header::JobArm>,
    /// Whether the header's background-jobs popover is open.
    pub(crate) jobs_open: bool,
    /// The job ids whose output panel is expanded inline.
    pub(crate) jobs_expanded: BTreeSet<String>,
    /// Cached output per expanded job, keyed by id and re-read only when the
    /// ring's length moves, so a frame never clones 64 KiB of output twice.
    pub(crate) jobs_output_cache: std::collections::HashMap<String, (usize, u64, String, bool)>,
    /// The running and settled counts the last frame resolved, and the roster
    /// version and session they were resolved for. A frame compares two values
    /// instead of walking the roster, which is what keeps the header cheap.
    pub(crate) jobs_counts: (usize, usize),
    jobs_counts_for: Option<String>,
    jobs_counts_version: u64,
    /// The composer command menu (the reference's add listbox).
    pub(crate) command_menu: super::command_menu::CommandMenu,
    /// The composer's slash palette: the grouped Add / Commands / Skills card.
    pub(crate) slash: super::slash_palette::SlashPalette,
    /// The inline note the composer is editing, if any.
    pub(crate) note: Option<super::slash_palette::NoteKind>,
    /// The note editor's text engine, built once at launch.
    pub(crate) note_input: Entity<InputState>,
    /// The window height as of the last frame, which caps the palette's list at
    /// about 60% of the surface it opens over.
    pub(crate) viewport_h: f32,
    /// The turn the rail was last scrolled to, so the follow fires once per
    /// change instead of every frame.
    rail_active: Option<usize>,
    /// The composer's current text, mirrored for the send button state.
    pub(crate) composer_value: String,
    /// The agent capabilities, read once when the settings dialog opens.
    pub(crate) caps: Vec<crate::caps::CapInfo>,
    pub(crate) plugins: Vec<crate::plugins::PluginInfo>,
    /// The settings dialog's text fields.
    pub(crate) settings_inputs: SettingsInputs,
    /// The Vault pane's text fields.
    pub(crate) vault_inputs: VaultInputs,
    /// The vault entry whose value is revealed inline, if any.
    pub(crate) vault_revealed: Option<String>,
    /// The vault entry awaiting a delete confirmation, if any.
    pub(crate) vault_confirm_delete: Option<String>,
    /// The last vault refusal, as the locale key the pane shows.
    pub(crate) vault_error: Option<&'static str>,
    /// Whether a key derivation is running, so the pane can say why it waits.
    pub(crate) vault_busy: bool,
    /// The server-name filter on the MCP tab.
    pub(crate) mcp_search: Entity<InputState>,
    /// What that field holds, kept beside the entity so the list can filter
    /// without reading the engine per row.
    pub(crate) mcp_query: String,
    /// The MCP server whose disclosure is open, by name. One at a time, the
    /// same rule the installed-plugin rows follow.
    pub(crate) mcp_open: Option<String>,
    /// Whether the MCP pane's add/edit form is open.
    pub(crate) mcp_form_open: bool,
    /// The entry the form edits, as the document and name it started from.
    pub(crate) mcp_editing: Option<(McpScope, String)>,
    /// Where a new entry is saved.
    pub(crate) mcp_scope: McpScope,
    /// Whether the form is on the HTTP transport.
    pub(crate) mcp_http: bool,
    /// The standing form refusal, as the locale key the pane shows.
    pub(crate) mcp_refusal: Option<&'static str>,
    /// The form's text fields: name, command, arguments and URL.
    pub(crate) mcp_name: Entity<InputState>,
    pub(crate) mcp_command: Entity<InputState>,
    pub(crate) mcp_args: Entity<InputState>,
    pub(crate) mcp_url: Entity<InputState>,
    /// Live subscriptions (input events).
    _subs: Vec<Subscription>,
}

/// Which route the add-provider card offers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ModelsAddRoute {
    /// One of the shipped providers, taking its family endpoint.
    Catalogue,
    /// A hand-typed provider id and endpoint.
    Custom,
}

/// One model row inside the open provider editor.
///
/// A row knows the nickname it is filed under in the tree, the leaf in effect
/// and the two text engines the reference's inputs read, so a keystroke can
/// repaint that row alone.
pub(crate) struct ModelRowEditor {
    /// The nickname this row is filed under in the provider's model map.
    pub(crate) key: String,
    /// The leaf in effect: model id, display name and declared input types.
    pub(crate) entry: crate::settings::global::ModelEntry,
    /// The model-id input.
    pub(crate) id: Entity<InputState>,
    /// The display-name input.
    pub(crate) name: Entity<InputState>,
    /// Whether this row's advanced part is open.
    pub(crate) open: bool,
}

/// Which measured width handle a drag is working.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum HandleSide {
    /// The handle left of the content column: dragging left widens it.
    Left,
    /// The handle right of the content column: dragging right widens it.
    Right,
}

/// A width drag in flight: the side being worked, the pointer x it started at
/// and the width the column had then - origin and base in the reference.
#[derive(Clone, Copy)]
pub(crate) struct ChatDrag {
    pub(crate) side: HandleSide,
    pub(crate) origin_x: f32,
    pub(crate) base: f32,
    /// Whether the pointer ever moved: the reference commits only a real drag.
    pub(crate) moved: bool,
}

/// A sidebar resize in flight: the reference records the pointer x and the
/// column width at pointerdown and then works from base + dx.
#[derive(Clone, Copy)]
pub(crate) struct SideDrag {
    pub(crate) origin_x: f32,
    pub(crate) base: f32,
    pub(crate) moved: bool,
}

/// The changed-file row the pointer is over.
///
/// The reference's hover card opens after a 500ms delay on its anchor, so the
/// row itself is not enough state: the card has to know WHEN the pointer
/// arrived to know whether the delay has passed, and where the pointer is so the
/// card can hang off it.
#[derive(Clone, Copy)]
pub(crate) struct ChangesHover {
    /// The message whose card the row belongs to.
    pub(crate) message: usize,
    /// The row's ordinal within that card's file list.
    pub(crate) file: usize,
    /// The pointer's window position when it arrived.
    pub(crate) at: (f32, f32),
}

/// One message's changed-file summary, and the steps it was built from.
///
/// A turn appends steps while it streams, so the entry is keyed on how many
/// steps the message has and how many argument bytes they carry together -
/// cheap to compute and enough to notice a step arriving or filling in. Without
/// it every frame re-parsed the arguments of every visible write.
#[derive(Default)]
pub(crate) struct ChangesCache {
    entries: std::collections::HashMap<usize, (usize, Vec<super::transcript::ChangedFile>)>,
}

impl ChangesCache {
    /// The summary for one message, rebuilt only when its steps changed.
    pub(crate) fn get(
        &mut self,
        message: usize,
        tools: &[crate::model::ToolEvent],
    ) -> std::rc::Rc<Vec<super::transcript::ChangedFile>> {
        let stamp = tools.len() * 4096 + tools.iter().map(|tool| tool.args.len()).sum::<usize>();
        let fresh = match self.entries.get(&message) {
            Some((cached, files)) if *cached == stamp => files.clone(),
            _ => {
                let files = super::transcript::turn_changes(tools);
                self.entries.insert(message, (stamp, files.clone()));
                files
            }
        };
        std::rc::Rc::new(fresh)
    }

    /// Drop everything: the message indices name rows in a transcript that
    /// just changed, so a session switch starts over.
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }
}

impl Shell {
    /// Build the view, wiring the composer's input events.
    ///
    /// `open_settings` opens the settings dialog as the window appears on the
    /// given rail index, which is what the `--settings` flag asks for.
    pub(crate) fn new(
        window: &mut Window,
        open_settings: Option<usize>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::assemble(window, open_settings, AppState::new(), cx)
    }

    /// Build the view over a supplied state. Tests point the store at a scratch
    /// directory through this, so no session file outside it is touched.
    #[cfg(test)]
    pub(crate) fn build_for_test(
        window: &mut Window,
        open_settings: Option<usize>,
        state: AppState,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::assemble(window, open_settings, state, cx)
    }

    /// The one place the shell is wired up.
    fn assemble(
        window: &mut Window,
        open_settings: Option<usize>,
        mut state: AppState,
        cx: &mut Context<Self>,
    ) -> Self {
        state.restore_session();
        // A workspace file written before the model became session-scoped
        // still names one. It is handed to the session that is now open, once,
        // so an upgrade does not strand the reader on "Select model".
        state.adopt_legacy_model();
        // MCP servers are committed configuration, so the live connections are
        // reconciled once at launch rather than on a render path.
        state.refresh_mcp();
        // Measured: the reference reads its stored content width
        // (dsh.conversation.contentWidth) on every width publish, so a window
        // reopens on the column the reader dragged.
        let chat_user_w = state.settings.global.general.content_width;
        let sidebar_user_w = state.settings.global.general.sidebar_width;
        let viewport_w = window.viewport_size().width.as_f32();
        // The meter's tools bucket follows the tool array a request carries, so
        // it is seeded from the same registry the next turn will send: without
        // this the bucket reads zero until the first turn runs.
        let choice = state
            .active_id
            .as_deref()
            .map(|id| state.session_choice(id))
            .unwrap_or_default();
        let built = crate::caps::build_agent(&state.settings, &state.vault, &choice, &state.mcp);
        state.set_tools_schema(built.tools_schema_chars);
        state.set_policy(built.policy.clone());
        // The tree order the sidebar last chose, from the settings file.
        state.sort =
            crate::store::SessionSort::from_persisted(&state.settings.global.general.session_sort);
        let lang = state.lang;
        // Measured: the hero editor draws its own placeholder, so the phase
        // the restore landed in picks the string.
        let hero = state.hero_phase();
        // Plan mode outranks the blank/hero phase: the composer asks for a task
        // to plan while the mode is on.
        let placeholder_key = if state.active_plan_mode() {
            "composer.planPlaceholder"
        } else if hero {
            "composer.heroPlaceholder"
        } else {
            "composer.placeholder"
        };
        let placeholder = placeholder_key;
        // Measured: the composer is a soft-wrapping editor that grows with its
        // content behind a 336px ceiling, and Enter submits while Shift+Enter
        // opens a line - which is the kit's own rule once the field is multi
        // line and `submit_on_enter` holds.
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(crate::i18n::tr(lang, placeholder))
                .auto_grow(1, crate::theme::COMPOSER_INPUT_MAX_ROWS)
                .submit_on_enter(true)
        });
        // The queue row's editor shares that shape: the reference's own
        // `.editor` is a textarea capped at 127px.
        let queue_edit = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, crate::theme::QUEUE_EDITOR_MAX_ROWS)
                .submit_on_enter(true)
        });
        // A review note is prose, so the field grows: the reference's card is a
        // wrapping textarea, not a one-line input.
        let files_note = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(crate::i18n::tr(lang, "right.files.notePlaceholder"))
        });
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::tr(lang, "side.search"))
        });
        let plugin_search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::tr(lang, "settings.searchPlugins"))
        });
        let policy_entry = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::tr(lang, "settings.policyAddHint"))
        });
        let mcp_search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::tr(lang, "settings.mcpSearch"))
        });
        let market_search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::tr(lang, "settings.marketSearch"))
        });
        let market_url = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::tr(lang, "settings.marketUrlHint"))
        });
        let skill_search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::tr(lang, "settings.skillsSearch"))
        });
        let skill_add_path = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::i18n::tr(lang, "settings.skillsPathHint"))
        });
        let skill_add_name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::i18n::tr(lang, "settings.skillsNameHint"))
        });
        let traj_search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::tr(lang, "traj.search"))
        });
        let audit_search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::tr(lang, "audit.search"))
        });
        let picker_search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::tr(lang, "settings.fetchSearch"))
        });
        let task_search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::tr(lang, "task.searchPlaceholder"))
        });
        let task_name = cx.new(|cx| InputState::new(window, cx));
        let task_prompt = cx.new(|cx| InputState::new(window, cx));
        let task_every = cx.new(|cx| InputState::new(window, cx));
        let mut subs = vec![
            cx.subscribe_in(
                &composer,
                window,
                |this: &mut Shell,
                 _state: &Entity<TextareaState>,
                 event: &InputEvent,
                 window: &mut Window,
                 cx: &mut Context<Shell>| match event {
                    InputEvent::Change => {
                        this.composer_value = this.composer.read(cx).value().to_string();
                        // A completed mention typed by hand becomes a chip the
                        // moment it is finished, so the draft reads the way the
                        // reference's does while the reader is still in it. The
                        // rewrite is skipped when nothing changed, which keeps it
                        // off the typing path for ordinary prose.
                        this.chip_open_draft(window, cx);
                        // The overlay follows the draft: a LEADING `/` opens the
                        // slash palette, a mid-draft `/` token shows the command
                        // source, and an `@` token the reference source.
                        this.slash.sync(this.composer_value.as_str(), &this.state);
                        this.command_menu.sync(this.composer_value.as_str());
                        cx.notify();
                    }
                    InputEvent::PressEnter {
                        secondary,
                        shift: false,
                        ..
                    } => {
                        // Cmd/Ctrl+Enter steers every waiting message into the
                        // running turn - the reference's accelerated submit -
                        // under its own gate: a running turn, a non-empty
                        // queue, an empty composer and no command card. When
                        // the gate does not hold, the same chord is the
                        // ordinary submit it was before steering existed.
                        if *secondary && this.steer_all_queued(cx) {
                            return;
                        }
                        // The reference gives Enter to the open overlay first: the
                        // highlighted row runs, and the composer keeps its draft.
                        if this.slash.open {
                            this.run_slash_row(this.slash.active, window, cx);
                        } else if this.command_menu.open {
                            this.run_row(this.command_menu.active, window, cx);
                        } else if this.run_command_line(window, cx) {
                            // A typed /plan or /goal line is a command, not a
                            // message; its output stays in the UI.
                        } else {
                            this.send(window, cx);
                        }
                    }
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &queue_edit,
                window,
                |this: &mut Shell,
                 _state: &Entity<TextareaState>,
                 event: &InputEvent,
                 _window: &mut Window,
                 cx: &mut Context<Shell>| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { shift: false, .. } => this.save_queue_edit(cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &search,
                window,
                |this: &mut Shell,
                 _state: &Entity<InputState>,
                 event: &InputEvent,
                 _window: &mut Window,
                 cx: &mut Context<Shell>| {
                    if matches!(event, InputEvent::Change) {
                        this.state.search_query = this.search.read(cx).value().to_string();
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(
                &plugin_search,
                window,
                |this: &mut Shell,
                 _state: &Entity<InputState>,
                 event: &InputEvent,
                 _window: &mut Window,
                 cx: &mut Context<Shell>| {
                    if matches!(event, InputEvent::Change) {
                        this.state.plugin_query = this.plugin_search.read(cx).value().to_string();
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(
                &mcp_search,
                window,
                |this: &mut Shell,
                 _state: &Entity<InputState>,
                 event: &InputEvent,
                 _window: &mut Window,
                 cx: &mut Context<Shell>| {
                    if matches!(event, InputEvent::Change) {
                        this.mcp_query = this.mcp_search.read(cx).value().to_string();
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(
                &market_search,
                window,
                |this: &mut Shell,
                 _state: &Entity<InputState>,
                 event: &InputEvent,
                 _window: &mut Window,
                 cx: &mut Context<Shell>| {
                    if matches!(event, InputEvent::Change) {
                        this.market_query = this.market_search.read(cx).value().to_string();
                        // A new search starts the window over: keeping a grown
                        // limit would render hundreds of rows for a new query.
                        this.market_limit = 60;
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(
                &skill_search,
                window,
                |this: &mut Shell,
                 _state: &Entity<InputState>,
                 event: &InputEvent,
                 _window: &mut Window,
                 cx: &mut Context<Shell>| {
                    if matches!(event, InputEvent::Change) {
                        this.skill_query = this.skill_search.read(cx).value().to_string();
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(
                &traj_search,
                window,
                |this: &mut Shell,
                 _state: &Entity<InputState>,
                 event: &InputEvent,
                 _window: &mut Window,
                 cx: &mut Context<Shell>| {
                    if matches!(event, InputEvent::Change) {
                        this.traj_query = this.traj_search.read(cx).value().to_string();
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(
                &audit_search,
                window,
                |this: &mut Shell,
                 _state: &Entity<InputState>,
                 event: &InputEvent,
                 _window: &mut Window,
                 cx: &mut Context<Shell>| {
                    if matches!(event, InputEvent::Change) {
                        this.audit_query = this.audit_search.read(cx).value().to_string();
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(
                &picker_search,
                window,
                |this: &mut Shell,
                 _state: &Entity<InputState>,
                 event: &InputEvent,
                 _window: &mut Window,
                 cx: &mut Context<Shell>| {
                    if matches!(event, InputEvent::Change) {
                        this.state.picker_query = this.picker_search.read(cx).value().to_string();
                        cx.notify();
                    }
                },
            ),
        ];
        // One text engine per provider form field. They wake the pane through
        // an `InputEvent::Change` subscription, so a disabled state an id already
        // taken produces repaints as the reader types.
        let add_route = cx.new(|cx| InputState::new(window, cx));
        let add_display = cx.new(|cx| InputState::new(window, cx));
        let add_base_url = cx.new(|cx| InputState::new(window, cx));

        // The models pane's add row: the id field carries the pane's own hint,
        // which is the only place the bare add row says what it takes.
        let model_id_hint = crate::i18n::tr(lang, "settings.modelId");
        let model_name_hint = crate::i18n::tr(lang, "settings.providerDisplayName");
        let new_model_id =
            cx.new(|cx| InputState::new(window, cx).placeholder(model_id_hint.clone()));
        let new_model_name =
            cx.new(|cx| InputState::new(window, cx).placeholder(model_name_hint.clone()));
        // The add row's optional display-name field appears once the id has
        // text, so the row stays one field until there is something to name.
        subs.push(cx.subscribe_in(
            &new_model_id,
            window,
            |_this: &mut Shell, _state, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            },
        ));
        let mcp_name = cx.new(|cx| InputState::new(window, cx));
        let mcp_command = cx.new(|cx| InputState::new(window, cx));
        let mcp_args = cx.new(|cx| InputState::new(window, cx));
        let mcp_url = cx.new(|cx| InputState::new(window, cx));
        let settings_inputs = SettingsInputs::new(window, cx);
        let vault_inputs = VaultInputs::new(window, state.lang, cx);
        // The passphrase fields repaint the strength and mismatch notes as they
        // are typed; the add row validates on its press, not per keystroke.
        for input in [&vault_inputs.passphrase, &vault_inputs.confirm] {
            subs.push(cx.subscribe_in(
                input,
                window,
                |this: &mut Shell,
                 _state: &Entity<InputState>,
                 event: &InputEvent,
                 _window: &mut Window,
                 cx: &mut Context<Shell>| {
                    if matches!(event, InputEvent::Change) {
                        this.vault_error = None;
                        cx.notify();
                    }
                },
            ));
        }
        let files_new = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::i18n::tr(state.lang, "right.files.newPlaceholder"))
        });
        let files_rename = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::i18n::tr(state.lang, "right.files.renamePlaceholder"))
        });
        // The session rename field: one engine, moved from row to row the way
        // the files pane's rename engine is, so a second rename reuses it.
        let session_rename = cx.new(|cx| InputState::new(window, cx));
        let workspace_rename = cx.new(|cx| InputState::new(window, cx));
        // The composer's inline note editor (the Goal and Feedback commands):
        // one engine, with its placeholder re-pointed when it opens.
        let note_input = cx.new(|cx| InputState::new(window, cx));
        subs.push(cx.subscribe_in(
            &note_input,
            window,
            |this: &mut Shell,
             _state: &Entity<InputState>,
             event: &InputEvent,
             window: &mut Window,
             cx: &mut Context<Shell>| {
                // Enter commits, which is the same contract the rename fields
                // follow. Escape is handled by the note card's key listener.
                if matches!(event, InputEvent::PressEnter { shift: false, .. }) {
                    this.commit_note(window, cx);
                }
            },
        ));
        subs.push(cx.subscribe_in(
            &workspace_rename,
            window,
            |this: &mut Shell,
             _state: &Entity<InputState>,
             event: &InputEvent,
             _window: &mut Window,
             cx: &mut Context<Shell>| {
                // Enter commits, and a blur deliberately does not cancel - the
                // same reason the session field does not (the menu that opened
                // it takes focus back in the frame it closes).
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.commit_workspace_rename(cx);
                }
            },
        ));
        subs.push(cx.subscribe_in(
            &session_rename,
            window,
            |this: &mut Shell,
             _state: &Entity<InputState>,
             event: &InputEvent,
             _window: &mut Window,
             cx: &mut Context<Shell>| {
                // Enter commits. A blur does NOT cancel: the field opens from a menu
                // item, and the menu's close (or the composer taking focus back)
                // blurs it in the same frame, which made the rename vanish the
                // instant it appeared. The field closes when the reader commits it
                // or opens another session.
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.commit_session_rename(cx);
                }
            },
        ));
        // The palette's keys are intercepted ABOVE the kit's input: the input
        // binds up, down, tab and escape in its own key context, so a raw key
        // listener on an ancestor never sees them. An interceptor runs before
        // action dispatch, which is the only seam that can claim those keys
        // while the card is open, and it is gated on the card being open and the
        // composer holding focus.
        let palette = cx.weak_entity();
        subs.push(cx.intercept_keystrokes(move |event, window, cx| {
            let Some(shell) = palette.upgrade() else {
                return;
            };
            if !shell.read(cx).composer_focused(window, cx) {
                return;
            }
            // The plus card is a listbox too: while it is open the arrows move
            // its highlight and Escape closes it, exactly as they do for the
            // slash palette. Without this the card answered no key at all.
            if !shell.read(cx).slash.open && shell.read(cx).command_menu.trigger.is_some() {
                let key = event.keystroke.key.as_str();
                if matches!(key, "up" | "down" | "escape") {
                    shell.update(cx, |this, cx| this.command_card_key(key, cx));
                    cx.stop_propagation();
                }
                return;
            }
            if !shell.read(cx).slash.open {
                return;
            }
            let keystroke = event.keystroke.clone();
            let handled = shell.update(cx, |this, cx| this.slash_key(&keystroke, cx));
            if handled == super::slash_palette::SlashKey::Handled {
                cx.stop_propagation();
            }
        }));
        let mut shell = Self {
            state,
            composer,
            placeholder_key,
            placeholder_fit: None,
            todo_open: false,
            queue_edit,
            queue_edit_for: None,
            queue_open: false,
            search,
            plugin_search,
            policy_entry,
            preset_details: None,
            preset_editor: None,
            preset_editor_error: None,
            market: Vec::new(),
            market_loaded: false,
            market_error: None,
            market_search,
            market_query: String::new(),
            market_limit: 60,
            market_url,
            market_fetching: 0,
            market_fetch_error: None,
            market_installing: None,
            market_install_error: None,
            market_sources_open: false,
            skills_save_error: None,
            skill_search,
            skill_query: String::new(),
            skill_add_open: false,
            skill_add_path,
            skill_add_name,
            traj_search,
            traj_actual_duration: false,
            // ON the clock from the start: equal width is about how WIDE a
            // block is drawn, never about whether it has a position. A false
            // time flag selects `sequence`, which packs a lane into a solid bar.
            traj_actual_time: true,
            traj_turns_collapsed: false,
            traj_calls_collapsed: false,
            traj_summary_hover: false,
            traj_select: None,
            traj_tab: 0,
            traj_query: String::new(),
            traj_lane_w: std::rc::Rc::new(std::cell::Cell::new(None)),
            audit_search,
            audit_mode: 0,
            audit_query: String::new(),
            audit_turn_open: BTreeSet::new(),
            audit_open: BTreeSet::new(),
            audit_full: BTreeSet::new(),
            picker_search,
            task_search,
            task_name,
            task_prompt,
            task_every,
            task_filter: 0,
            task_delete: None,
            permission_confirm: None,
            trust_confirm: None,
            trust_error: None,
            permission_acknowledged: false,
            developer_tools_failed: false,
            shortcuts_open: false,
            config_open_failed: false,
            models_add_open: false,
            models_add_route: ModelsAddRoute::Catalogue,
            add_route,
            add_display,
            add_base_url,
            add_key_vault: None,
            model_rows: Vec::new(),
            new_model_id,
            new_model_name,
            models_advanced_open: false,
            catalogue_choice: None,
            catalogue_open: false,
            composer_value: String::new(),
            caps: Vec::new(),
            plugins: Vec::new(),
            settings_inputs,
            vault_inputs,
            vault_revealed: None,
            vault_confirm_delete: None,
            vault_error: None,
            vault_busy: false,
            mcp_search,
            mcp_query: String::new(),
            mcp_open: None,
            mcp_form_open: false,
            mcp_editing: None,
            mcp_scope: McpScope::Global,
            mcp_http: false,
            mcp_refusal: None,
            mcp_name,
            mcp_command,
            mcp_args,
            mcp_url,
            transcript_scroll: ScrollHandle::new(),
            transcript_layout: std::cell::RefCell::new(
                super::transcript::TranscriptLayout::default(),
            ),
            body_states: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            frame_no: 0,
            tail_max: None,
            tail_session: None,
            following_tail: true,
            audit_jump_to: None,
            think_open: std::collections::HashSet::new(),
            compaction_open: std::collections::HashSet::new(),
            chat_last_offset: 0.0,
            traj_last_offset: 0.0,
            chat_smooth: super::smooth::Smooth::default(),
            traj_smooth: super::smooth::Smooth::default(),
            traj_scroll: ScrollHandle::new(),
            audit_scroll: ScrollHandle::new(),
            side_scroll: ScrollHandle::new(),
            side_focus: cx.focus_handle(),
            rail_scroll: ScrollHandle::new(),
            settings_scroll: ScrollHandle::new(),
            rail_hover: Some(58),
            copied_message: None,
            selection_start: None,
            changes_open: std::collections::HashSet::new(),
            changes_hover: None,
            changes_preview: None,
            changes_cache: std::cell::RefCell::new(ChangesCache::default()),
            rail_active: None,
            conversation_w: px(0.0),
            chat_user_w,
            sidebar_user_w,
            viewport_w,
            jobs_arm: None,
            jobs_open: false,
            jobs_expanded: BTreeSet::new(),
            jobs_output_cache: std::collections::HashMap::new(),
            jobs_counts: (0, 0),
            jobs_counts_for: None,
            jobs_counts_version: 0,
            command_menu: super::command_menu::CommandMenu::default(),
            slash: super::slash_palette::SlashPalette::default(),
            note: None,
            note_input,
            viewport_h: window.viewport_size().height.as_f32(),
            chat_drag: None,
            chat_hover: None,
            sidebar_drag: None,
            renaming_session: None,
            session_rename,
            renaming_workspace: None,
            workspace_rename,
            trash_open: false,
            installed_open: std::collections::BTreeSet::new(),
            installed_cache: crate::installed::InstalledCache::default(),
            trace_rows: Vec::new(),
            trace_session: None,
            trace_pages: 0,
            trace: PageFlow::default(),
            trace_done: false,
            older_anchor: None,
            older: PageFlow::default(),
            compaction_checked: std::collections::HashSet::new(),
            plugins_tab: 0,
            skills_browse_open: false,
            skills_dirs_open: false,
            skill_filter: SkillFilter::All,
            trash_entries: Vec::new(),
            terminal_snapshot: std::rc::Rc::new(crate::terminal::Snapshot::default()),
            terminal_version: 0,
            terminal_focus: cx.focus_handle(),
            terminal_frame: std::rc::Rc::new(std::cell::Cell::new(None)),
            right_hover: false,
            right: super::right_panel::RightPanel::default(),
            files: super::right_panel::FilesPane::new(files_new, files_rename, files_note),
            models_fetched: None,
            models_fetch_failed: false,
            models_fetch_needs_base_url: false,
            models_fetching: false,
            models_selected: std::collections::BTreeSet::new(),
            _subs: subs,
        };
        crate::theme::apply(&shell.state.settings.global.general.theme_mode, window, cx);
        // Follow a live host appearance switch, which is what makes the System
        // option mean anything: the copied palette tokens would otherwise go
        // stale the moment macOS changed.
        shell
            ._subs
            .push(cx.observe_window_appearance(window, |this, window, cx| {
                this.appearance_changed(window, cx);
            }));
        // The terminal clock: the PTY's reader thread fills the scrollback on
        // its own, so the pane takes a copy whenever the version moved. A quiet
        // terminal costs one compare, which is why the cadence can be fine.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(120))
                .await;
            if this
                .update(cx, |this, cx| {
                    // The policy file's stamp is checked here, not per frame:
                    // an edited policy takes effect without a restart.
                    this.refresh_terminal_policy(cx);
                    this.sync_terminal(cx);
                })
                .is_err()
            {
                break;
            }
        })
        .detach();
        // The jobs clock: the reference re-renders its elapsed figures once a
        // second while the panel is open over a live job, and bsh needs the same
        // cadence to expire an armed stop inside its measured window.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(1))
                .await;
            if this
                .update(cx, |this, cx| {
                    this.tick_jobs(std::time::Instant::now(), cx);
                })
                .is_err()
            {
                break;
            }
        })
        .detach();
        // A child turn streams outside the parent's event channel, so the
        // subagent rows and the selected transcript need their own fast clock.
        // It is idle-cheap: the update is a lock check and a notify only while
        // a child is actually running.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(100))
                .await;
            if this
                .update(cx, |this, cx| {
                    if this.state.subagents.running_count() == 0 {
                        return;
                    }
                    if this.state.subagent.is_some() {
                        this.state.refresh_subagent_preview();
                    }
                    cx.notify();
                })
                .is_err()
            {
                break;
            }
        })
        .detach();
        // The scheduler: one check every 30 seconds, so a task comes due while the
        // window is open without needing a turn of its own to hang off.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(30))
                .await;
            if this
                .update(cx, |this, cx| {
                    if !this.state.settings.global.schedules.is_empty() {
                        this.check_schedules(cx);
                    }
                })
                .is_err()
            {
                break;
            }
        })
        .detach();
        if let Some(section) = open_settings {
            shell.open_settings_at(section, window, cx);
        }
        // Opening the restored session is the one moment a legacy log is
        // checked; the rewrite itself runs on the background executor.
        shell.maybe_compact_history(cx);
        shell
    }

    /// React to the host appearance changing.
    ///
    /// Only the System mode follows it; a pinned Light or Dark keeps the palette
    /// the user chose, so this is the gate the option promises.
    /// Fetch the endpoint model list through the react vendor seam.
    ///
    /// The reference gates this on a base URL, so an empty one is a hint and
    /// not a request. The HTTP call runs on the shell tokio runtime because
    /// the gpui executor has no reactor, and only the answer comes back to the
    /// view; a refusal is reported rather than painted as an empty list.
    pub(crate) fn fetch_models(&mut self, provider: &str, cx: &mut Context<Self>) {
        let Some(node) = self.state.settings.global.providers.get(provider).cloned() else {
            return;
        };
        let endpoint = node.base_url.trim().to_string();
        if endpoint.is_empty() {
            self.models_fetch_needs_base_url = true;
            cx.notify();
            return;
        }
        let model = node
            .models
            .values()
            .find(|leaf| !leaf.model.trim().is_empty())
            .map(|leaf| leaf.model.trim().to_string())
            .unwrap_or_default();
        // The vault is the only credential source, so the fetch resolves the
        // same entry the agent build will: the chosen id, else the conventional
        // provider:<id>:api_key entry.
        let (api_key, _) = crate::caps::provider_credential(
            provider,
            node.api_key_vault.as_deref(),
            &self.state.vault,
        );
        let (tx, rx) = async_channel::bounded(1);
        self.state.runner.rt().spawn(async move {
            let vendor = react::llm::vendor::OpenAiVendor::new(endpoint, model, api_key);
            let _ = tx.send(vendor.fetch_model_ids().await).await;
        });
        self.models_fetch_failed = false;
        self.models_fetch_needs_base_url = false;
        self.models_fetching = true;
        cx.spawn(async move |this, cx| {
            let outcome = rx.recv().await;
            let _ = this.update(cx, |this, cx| {
                match outcome {
                    Ok(Ok(models)) => {
                        this.models_fetched = Some(models);
                        this.models_selected.clear();
                    }
                    _ => {
                        this.models_fetched = None;
                        this.models_fetch_failed = true;
                    }
                }
                // The ask is over whether it answered or refused, so the
                // trigger goes back to naming the request.
                this.models_fetching = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// Tick or untick one fetched model in the picker.
    pub(crate) fn toggle_fetched_model(&mut self, model: &str) {
        if !self.models_selected.remove(model) {
            self.models_selected.insert(model.to_string());
        }
    }

    /// Add every ticked fetched model to the active provider catalog.
    ///
    /// The catalog itself refuses blanks and repeats, so adding twice is
    /// harmless; the picker closes because the reference dialog does.
    pub(crate) fn add_selected_models(
        &mut self,
        provider: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ids: Vec<String> = self.models_selected.iter().cloned().collect();
        let draft = self.state.draft_mut();
        for id in ids {
            let _ = draft.global.add_model(provider, &id, &id, &id, "");
        }
        self.models_selected.clear();
        self.models_fetched = None;
        self.reload_model_rows(provider, window, cx);
        cx.notify();
    }

    pub(crate) fn appearance_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.draft().global.general.theme_mode == "system" {
            crate::theme::apply("system", window, cx);
            cx.notify();
        }
    }

    /// Drop one pending attachment.
    pub(crate) fn remove_attachment(&mut self, index: usize) {
        if index < self.state.attachments.len() {
            self.state.attachments.remove(index);
        }
    }

    /// Ask for files and mention them in the composer.
    pub(crate) fn attach_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let choice = self
            .state
            .active_id
            .as_deref()
            .map(|id| self.state.session_choice(id))
            .unwrap_or_default();
        let accepts = self.state.settings.accepts_image_for(&choice);
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("Attach to the message".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let mut picked: Vec<crate::state::Attachment> = Vec::new();
            let mut mention = String::new();
            let mut refused = false;
            for path in &paths {
                match crate::state::Attachment::from_path(path) {
                    Some(_) if !accepts => refused = true,
                    Some(attachment) => picked.push(attachment),
                    None => mention.push_str(&format!(" @{}", path.display())),
                }
            }
            this.update_in(cx, |this, window, cx| {
                if refused {
                    this.state.notice(
                        crate::i18n::tr(this.state.lang, "composer.imageRefused").to_string(),
                    );
                }
                this.state.attachments.extend(picked);
                if !mention.is_empty() {
                    let current = this.composer.read(cx).value().to_string();
                    let next = format!("{current}{mention}");
                    // Appending a picked path is an INSERT: the caret belongs
                    // after it, not back at the draft's start.
                    this.set_composer_text(&next, window, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Ask for a folder and add it as a workspace.
    ///
    /// A folder that is already a workspace is selected instead of added
    /// twice, so the button is safe to press on a root the user already has.
    pub(crate) fn add_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt: SharedString = self.state.lang.tr("side.addWorkspace").into();
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(prompt),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.first() else {
                return;
            };
            this.update_in(cx, |this, _window, cx| {
                this.adopt_workspace(path, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Register a directory as a workspace and switch to it.
    ///
    /// The half of [`Shell::add_workspace`] that does not need a dialog, so the
    /// hero's "Add workspace..." row and the sidebar's button land in the same
    /// place - and so a test can drive it without a picker.
    pub(crate) fn adopt_workspace(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        let root = path.to_string_lossy().to_string();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| root.clone());
        let existing = self
            .state
            .settings
            .global
            .workspaces
            .iter()
            .find(|workspace| workspace.root == root)
            .map(|workspace| workspace.id.clone());
        let id = existing.unwrap_or_else(|| self.state.adopt_workspace(&name, &root));
        self.state.settings.global.general.active_workspace = id;
        let _ = self.state.settings.save();
        // A new workspace means a new tree and new notes, and the open file
        // tabs belonged to the old root.
        self.refile_workspace(cx);
        cx.notify();
    }

    /// Open the settings dialog on a given rail section.
    pub(crate) fn open_settings_at(
        &mut self,
        section: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings(window, cx);
        // The MCP view is a TAB of the Extensions pane, not a rail cell of its
        // own. Landing on the old section index drew the MCP body with no tab
        // strip and no selected rail row, which is not the pane the reader sees
        // after two clicks. Map the deep link onto Extensions + the MCP tab.
        let mut section = section;
        if Some(section) == super::settings::section_index("mcp") {
            self.plugins_tab = 1;
            if let Some(plugins) = super::settings::section_index("plugins") {
                section = plugins;
            }
        }
        self.state.settings_section = section;
    }

    /// Open the settings dialog, loading the fields from the saved settings.
    pub(crate) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.open_settings();
        self.preset_editor = None;
        self.preset_editor_error = None;
        self.preset_details = None;
        // The Skills filter is view state, not a preference: every open starts
        // from All, so a reader never lands on a pane that hides half the
        // catalogue behind a choice they made on an earlier visit.
        self.skill_filter = SkillFilter::All;
        // A write failure belongs to the visit that saw it, like the filter.
        self.skills_save_error = None;
        // The dialog opens on a fresh read of the installed packages: a package
        // may have been installed, removed or upgraded by another process since
        // the cache was filled, and this is a user action, not a render.
        self.installed_cache.clear();
        let draft = self.state.draft().clone();
        self.settings_inputs.sync(&draft, window, cx);
        cx.notify();
    }

    /// Close the settings dialog, discarding everything it edited.
    ///
    /// The draft is what holds the edits, so dropping it is most of the work.
    /// Two preferences are also PREVIEWED while the dialog is open - the
    /// interface language and the palette - and those live outside the draft,
    /// so they are put back from the committed settings here. Without this a
    /// cancelled dialog kept the language it had merely been trying on.
    pub(crate) fn cancel_settings(&mut self, cx: &mut Context<Self>) {
        self.state.cancel_settings();
        self.preset_editor = None;
        self.preset_editor_error = None;
        self.preset_details = None;
        let committed = &self.state.settings.global.general;
        self.state.lang = crate::i18n::Lang::from_locale(&committed.lang);
        let mode = committed.theme_mode.clone();
        crate::theme::apply_here(&mode, cx);
        self.developer_tools_failed = false;
        cx.notify();
    }

    /// Keep the dialog's settings, persist them, and rebuild the capability list.
    ///
    /// The dialog stays open: Save is a commit, not a close. The draft is taken
    /// by the commit, so a second Save starts from the committed document plus
    /// the field values - never from `Settings::default()`, which would wipe
    /// every pane the fields do not cover.
    pub(crate) fn save_settings(&mut self, cx: &mut Context<Self>) {
        let mut draft = self
            .state
            .draft
            .clone()
            .unwrap_or_else(|| self.state.settings.clone());
        self.settings_inputs.read(&mut draft, cx);
        self.state.draft = Some(draft);
        match self.state.commit_settings() {
            Ok(()) => {
                self.preset_editor = None;
                self.preset_editor_error = None;
                self.preset_details = None;
                self.caps.clear();
                self.plugins.clear();
                // A save can change the inventory (an installed package added
                // or removed in the draft), so the next list re-reads packages.
                self.installed_cache.clear();
                self.developer_tools_failed = false;
                // A full Save supersedes any earlier focused-write refusal.
                self.skills_save_error = None;
                self.state
                    .notice(crate::i18n::tr(self.state.lang, "settings.saved").to_string());
            }
            Err(error) => {
                // The inline error beside the developer-tools row and the
                // notice say the same thing, so a file that could not be
                // written is visible where the reader was looking.
                self.developer_tools_failed = true;
                self.state.notice(format!(
                    "{} {error}",
                    crate::i18n::tr(self.state.lang, "settings.saveFailed")
                ));
            }
        }
        cx.notify();
    }

    /// The one reveal path: the header action and the account pane's button
    /// both land here, so the inline error and the notice cannot disagree.
    pub(crate) fn open_config(&mut self, cx: &mut Context<Self>) {
        super::settings::reveal_config_shared(self, cx);
    }

    /// The claim hint for a command token that fills the draft, which is the
    /// reference's `hint.<command>` placeholder.
    ///
    /// It shows only while the token is the WHOLE input, so typing the argument
    /// returns the placeholder to the ordinary one.
    fn claim_hint(&self) -> Option<&'static str> {
        let text = self.composer_value.trim();
        if text == "/plan" {
            return Some("hint.plan");
        }
        if text == "/goal" {
            return Some(
                if self
                    .state
                    .active_session()
                    .and_then(|session| session.goal.as_ref())
                    .is_some()
                {
                    "hint.goal.active"
                } else {
                    "hint.goal"
                },
            );
        }
        None
    }

    /// Send the composer's text, or queue it while the session is running.
    pub(crate) fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        if let Some(id) = self
            .state
            .subagent
            .as_ref()
            .map(|subagent| subagent.id.clone())
        {
            self.send_to_subagent(id, text, window, cx);
            return;
        }
        let attachments = std::mem::take(&mut self.state.attachments);
        match self.submit(text, attachments, cx) {
            Submit::Accepted => self.clear_composer(window, cx),
            // A refusal leaves the chips where the reader can retry them.
            Submit::Refused(attachments) => self.state.attachments = attachments,
        }
    }

    /// Put one message into the session: queue it behind a running turn, or
    /// open a turn when the session is idle.
    ///
    /// The composer and a queue row's Send now both come through here, so
    /// pressing the arrow on an idle session behaves exactly as if the reader
    /// had submitted the text. On refusal the attachments come back, because
    /// the caller knows where they belong: the composer's chip row, or the
    /// queue row that must not lose them.
    fn submit(
        &mut self,
        text: String,
        attachments: Vec<crate::state::Attachment>,
        cx: &mut Context<Self>,
    ) -> Submit {
        // Measured: the reference queues a submission while its session is
        // already running instead of opening a second turn over the same
        // transcript, which is what its queue dock lists.
        if let Some(id) = self.state.active_id.clone() {
            // A running turn is the reference's ordinary queue case. A manual
            // compaction is the second: its own prompts are "accepted and start
            // after it finishes", so a submission made while the summarizer
            // works must wait rather than open a turn under it.
            if self.state.is_streaming(&id) || self.state.is_compacting(&id) {
                self.state.enqueue(&id, text, attachments);
                return Submit::Accepted;
            }
        }
        // Plan mode and any @reference guidance travel on the REQUEST, not on
        // the transcript: the reader's line stays what they typed. The goal is
        // NOT put on an ordinary request - the reference keeps goal state out of
        // model context and sends it only in a goal round's prompt.
        let plan = self.state.active_plan_mode();
        let request = crate::state::turn_text(&text, plan);
        let input = crate::state::turn_input(&request, &attachments);
        let display = crate::state::display_text(&text, &attachments);
        // The session on screen is the one this send opens a turn in, so its
        // own model choice is what the gate and the build resolve.
        let choice = self
            .state
            .active_id
            .as_deref()
            .map(|id| self.state.session_choice(id))
            .unwrap_or_default();
        // A name the trees no longer define is refused where the reader can see
        // it, before a turn opens against a model nobody chose.
        if let Some(key) = self.state.settings.refusal_for(&choice) {
            let why = crate::i18n::tr(self.state.lang, key);
            self.state.notice(why);
            return Submit::Refused(attachments);
        }
        // A credential the vault was meant to supply but cannot, because the
        // vault is locked, is refused here - before a turn opens - rather than
        // handed to the provider as an empty key.
        if let Some(error) =
            crate::caps::credential_error(&self.state.settings, &self.state.vault, &choice)
        {
            let why = error.message(self.state.lang);
            self.state.notice(why);
            return Submit::Refused(attachments);
        }
        let Some((id, transcript, budget)) = self.state.begin_turn(&display) else {
            return Submit::Refused(attachments);
        };
        self.spawn_turn(id, input, transcript, budget, cx);
        Submit::Accepted
    }

    /// Send one waiting message to the running turn now - the reference's
    /// Steer.
    ///
    /// The message leaves the dock and goes into the turn's next-step mailbox;
    /// the turn appends it as a user row at the step that takes it and reports
    /// the ticket back, so the transcript shows where the reader interjected.
    /// A session with no running turn has nothing to steer into, so the row is
    /// submitted normally instead: "send now" is then just a send.
    pub(crate) fn steer_now(&mut self, id: &str, seq: u64, cx: &mut Context<Self>) {
        if !self.state.is_streaming(id) {
            let Some(queued) = self.state.take_queued(id, seq) else {
                return;
            };
            match self.submit(queued.text.clone(), queued.attachments.clone(), cx) {
                Submit::Accepted => {}
                Submit::Refused(_) => self.state.restore_queued(id, queued),
            }
            cx.notify();
            return;
        }
        // No mailbox means the turn is between the store and the runner: keep
        // the row queued rather than dropping it into nothing.
        let Some(mailbox) = self.state.steers(id) else {
            return;
        };
        let Some(queued) = self.state.take_queued(id, seq) else {
            return;
        };
        let plan = self.state.plan_mode(id);
        let request = crate::state::turn_text(&queued.text, plan);
        let input = crate::state::turn_input(&request, &queued.attachments);
        let accepted = mailbox.push(agent::SteerMessage {
            content: input,
            ticket: seq,
        });
        if !accepted {
            // The turn is holding as many messages as it will take, so the row
            // goes back rather than disappearing into a mailbox no step reads.
            self.state.restore_queued(id, queued);
            let why = crate::i18n::tr(self.state.lang, "queue.steerFailed");
            self.state.notice(why);
            cx.notify();
            return;
        }
        self.state.mark_steering(id, queued);
        cx.notify();
    }

    /// Steer every waiting message, oldest first. True when there was anything
    /// to steer.
    ///
    /// The reference's gate: the chord steers only while a turn runs, a queue
    /// waits, the composer holds no draft and no command card is open;
    /// otherwise the same chord is the ordinary accelerated submit it was
    /// before steering existed.
    pub(crate) fn steer_all_queued(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.state.active_id.clone() else {
            return false;
        };
        // A child run owns the composer while it is selected, so the chord must
        // not reach past it into the parent session's queue - the reference's
        // own `steeringAvailable` term.
        if self.state.subagent.is_some()
            || !self.state.is_streaming(&id)
            || !self.state.has_queued(&id)
        {
            return false;
        }
        if !self.composer.read(cx).value().trim().is_empty() || !self.state.attachments.is_empty() {
            return false;
        }
        if self.slash.open || self.command_menu.trigger.is_some() {
            return false;
        }
        let seqs: Vec<u64> = self
            .state
            .inbox(&id)
            .iter()
            .map(|queued| queued.seq)
            .collect();
        for seq in seqs {
            self.steer_now(&id, seq, cx);
        }
        true
    }

    /// Whether the accelerated chord would steer right now.
    ///
    /// Exactly the condition the composer's placeholder states, so the promise
    /// and the key agree: a running turn with a message waiting behind it.
    pub(crate) fn queued_steerable(&self) -> bool {
        self.state.subagent.is_none()
            && self
                .state
                .active_id
                .as_deref()
                .is_some_and(|id| self.state.is_streaming(id) && self.state.has_queued(id))
    }

    /// Empty the composer after a submission left it.
    /// Continue the selected child run with a follow-up prompt.
    fn send_to_subagent(
        &mut self,
        id: String,
        prompt: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(agent) = self.state.subagents.agent(&id) else {
            self.state
                .notice(crate::i18n::tr(self.state.lang, "subagents.unavailable").to_string());
            return;
        };
        let Some(run) = self
            .state
            .subagents
            .snapshot()
            .into_iter()
            .find(|run| run.id == id)
        else {
            self.state
                .notice(crate::i18n::tr(self.state.lang, "subagents.unavailable").to_string());
            return;
        };
        // A follow-up is another child turn, so it asks the same spawn
        // question the tool asks. The check runs before `begin_followup`
        // mutates the run: a denial leaves the child exactly as it was.
        let eff = self.state.effective_in(&run.session);
        let policy = crate::caps::policy_for_effective(&self.state.settings, &eff);
        if let Err(refusal) =
            crate::caps::check_spawn(&policy, "subagent", &run.session, &eff.preset_id, &eff.root)
        {
            self.state.notice(refusal);
            return;
        }
        if !self.state.subagents.begin_followup(&id, &prompt) {
            self.state
                .notice(crate::i18n::tr(self.state.lang, "subagents.busy").to_string());
            return;
        }
        self.state.refresh_subagent_preview();
        self.state.persist_subagents();
        self.clear_composer(window, cx);
        let registry = self.state.subagents.clone();
        let stream_registry = self.state.subagents.clone();
        let stream_id = id.clone();
        // The engine awaits a Tokio timer for its LLM timeout; GPUI's executor
        // is not a Tokio runtime, so the child turn must be spawned on the
        // runner's runtime and only its result awaited from the UI task.
        let handle = self.state.runner.spawn_task(async move {
            let guard = agent.lock().await;
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(600),
                crate::caps::stream_child(&guard, prompt, stream_registry, &stream_id),
            )
            .await
            .map_err(|_| "subagent timed out".to_string())
            .and_then(|result| result);
            let usage = guard.last_token_usage().unwrap_or((0, 0));
            let transcript = crate::caps::child_transcript(&guard);
            (result, usage, transcript)
        });
        cx.spawn(async move |this, cx| {
            let outcome = handle.await;
            let _ = this.update(cx, |this, cx| {
                match outcome {
                    Ok((Ok((text, usage)), _, transcript)) => {
                        registry.finish(&id, text, usage, transcript);
                    }
                    Ok((Err(error), usage, transcript)) => {
                        registry.fail(&id, error, Some(usage), transcript);
                    }
                    Err(error) => {
                        registry.fail(
                            &id,
                            format!("subagent task ended: {error}"),
                            None,
                            Vec::new(),
                        );
                    }
                }
                this.state.refresh_subagent_preview();
                this.state.persist_subagents();
                cx.notify();
            });
        })
        .detach();
    }

    /// Condense the active session's older history into a model-written
    /// checkpoint, the way the reference's `/compact` does.
    ///
    /// The work is an auxiliary model request, not a turn: no tool is offered,
    /// nothing is appended to the transcript, and the reader can keep typing -
    /// a prompt sent meanwhile queues exactly as it does during a turn. The
    /// request runs on the runner's Tokio runtime, because the engine awaits a
    /// Tokio timer for its timeout and GPUI's executor has no reactor.
    ///
    /// Everything that can refuse does so before the request is built: no
    /// session, a streaming turn, no chosen model, no credential, or nothing
    /// old enough to fold. The checkpoint is installed only after the summary
    /// returns, so a failed or stopped request leaves the session untouched.
    /// Move the plus card's highlight with the arrow keys, or close it.
    ///
    /// The kit's textarea consumes arrows in its own key context, so the card
    /// is driven from the same app-level interceptor the slash palette uses.
    pub(crate) fn command_card_key(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(trigger) = self.command_menu.trigger else {
            return;
        };
        let rows = self.card_rows(trigger).len();
        if rows == 0 {
            return;
        }
        match key {
            "down" => {
                self.command_menu.active = (self.command_menu.active + 1) % rows;
            }
            "up" => {
                self.command_menu.active = (self.command_menu.active + rows - 1) % rows;
            }
            "escape" => {
                self.command_menu.close();
            }
            _ => {}
        }
        cx.notify();
    }

    pub(crate) fn compact_history(&mut self, cx: &mut Context<Self>) {
        let lang = self.state.lang;
        let Some(id) = self.state.active_id.clone() else {
            self.state.notice(lang.tr("slash.nothingToCompact"));
            cx.notify();
            return;
        };
        // Admission first, the reference's own order: compactNow "requires an
        // idle agent with no waking queued work", so a stream, another
        // compaction, or a prompt already waiting behind this session all
        // report busy before the selection runs.
        if self.state.is_streaming(&id)
            || self.state.is_compacting(&id)
            || self.state.has_queued(&id)
        {
            self.refuse_compact(&id, "slash.compactBusy", true, cx);
            return;
        }
        if !self.state.has_compactable_history(&id) {
            self.refuse_compact(&id, "slash.nothingToCompact", false, cx);
            return;
        }
        // The same gate a turn passes: compaction is a model call, so it needs
        // a chosen, credentialed model. Refusing here costs no request. The
        // reference's summarizer throws when no provider/model resolves, and
        // `compactNow` classifies that as a summary failure, so the command
        // reports the same text rather than inventing a fourth outcome.
        let choice = self.state.session_choice(&id);
        if self.state.settings.refusal_for(&choice).is_some() {
            self.refuse_compact(&id, "slash.compactSummary", true, cx);
            return;
        }
        let built = crate::caps::build_agent_for_session_with_subagents_and_jobs(
            &self.state.settings,
            &self.state.vault,
            &choice,
            &self.state.mcp,
            &id,
            &id,
            self.state.subagents.clone(),
            self.state.jobs.clone(),
        );
        if built.credential_error.is_some() {
            self.refuse_compact(&id, "slash.compactSummary", true, cx);
            return;
        }
        let agent = built.agent;
        // The manual command condenses everything safe, which is the
        // contract's own rule for `/compact`: it does not wait for pressure,
        // so it retains no minimum tail beyond a balanced cut.
        let retain = AppState::COMPACT_RETAIN;
        // The transcript the summarizer will carry, and the boundary that span
        // ends at. Both come from one reading of the NEXT REQUEST's window: a
        // replayed checkpoint plus the rows after `session.compacted`. Asking
        // the whole on-screen transcript would fold rows that left the context
        // long ago and land the boundary before the current one.
        let (messages, start, replayed) = {
            let Some(session) = self.state.store.session(&id) else {
                self.state.notice(lang.tr("slash.nothingToCompact"));
                cx.notify();
                return;
            };
            let messages = self.state.request_transcript(session);
            let replayed = usize::from(self.state.has_replayed_checkpoint(session));
            let mut probe = agent::agent::AgentSession::new();
            probe.restore_messages(crate::caps::agent_messages(&messages));
            let Some((removed, _)) = probe.split_for_compaction_at(retain) else {
                self.refuse_compact(&id, "slash.nothingToCompact", false, cx);
                return;
            };
            let start = AppState::compaction_boundary(session, removed.len(), replayed);
            (messages, start, replayed)
        };
        // The agent must carry the same history the boundary was computed from,
        // or the summarizer would fold a different span than the one installed.
        // This bridge is text-only: images in the span are not shown to the
        // summarizer and tool calls/results are display-only rows, so the
        // reference's image-offload and tool-pairing protections have no seam
        // here. Reported rather than faked.
        agent
            .session()
            .restore_messages(crate::caps::agent_messages(&messages));

        self.state.begin_compact_command(&id);
        self.state.start_compacting(&id);
        self.state.notice(lang.tr("slash.compacting"));
        cx.notify();

        let signal = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handle = self
            .state
            .runner
            .spawn_task(async move { agent.compact_now(retain, &signal).await });
        cx.spawn(async move |this, cx| {
            let outcome = handle.await;
            let _ = this.update(cx, |this, cx| {
                // The shipped taxonomy also distinguishes `changed`, `commit`
                // and `persistence`. bsh cannot produce them: the summary is
                // installed and saved in one synchronous step after the request
                // returns, there is no second writer that could rewrite the
                // selected span while it is summarized, and `save_session`
                // discards its error. Reporting one of those strings would be
                // inventing a failure the reader could never act on, so only
                // the reachable classes appear: `busy` (the gates above),
                // `cancelled` (the unreachable stop arm), and `summary`.
                let (text, error, landed) = match outcome {
                    // The command reports the SHADOWED span's price, which is
                    // what the shipped command's `shadowedTokenCount` names,
                    // not the net difference after the checkpoint is inserted.
                    Ok(Ok(Some(report))) => (
                        crate::i18n::tr_args(
                            lang,
                            "slash.compacted",
                            &[
                                ("count", report.replaced.to_string()),
                                ("tokens", report.before_tokens.to_string()),
                            ],
                        ),
                        false,
                        Some(crate::model::CompactionReport {
                            // A replayed checkpoint is a synthetic head, not a
                            // transcript row, so it does not inflate the count
                            // the marker reports.
                            items: report.replaced.saturating_sub(replayed),
                            tokens: report.before_tokens,
                            summary: report.summary,
                            manual: true,
                        }),
                    ),
                    Ok(Ok(None)) => (lang.tr("slash.nothingToCompact"), false, None),
                    // Cancellation is unreachable today - the command exposes no
                    // stop control and the signal is never set - but the arm is
                    // here so a future cancel seam cannot be misreported as a
                    // summary failure.
                    Ok(Err(agent::AgentError::Stopped)) => {
                        (lang.tr("slash.compactCancelled"), true, None)
                    }
                    Ok(Err(_)) | Err(_) => (lang.tr("slash.compactSummary"), true, None),
                };
                if let Some(report) = landed {
                    // A landed checkpoint replaces the command row, which is
                    // the reference's `CompactionCommandCard` rule; the marker
                    // above the boundary is what the reader keeps instead.
                    if this.state.land_manual_compaction(&id, start, report) == 0 {
                        let text = lang.tr("slash.nothingToCompact");
                        this.state.finish_compact_command(&id, text.clone(), false);
                    }
                } else {
                    this.state.finish_compact_command(&id, text.clone(), error);
                }
                this.state.stop_compacting(&id);
                this.state.notice(text);
                // Prompts sent while the summarizer ran are accepted and start
                // after it finishes, which is the reference's inbox rule.
                this.drain_inbox(&id, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Report one `/compact` outcome that never opened a request.
    ///
    /// The reference returns these from the command handler, so they are
    /// `command/done` rows: the transcript keeps the card and the outcome text.
    fn refuse_compact(&mut self, id: &str, key: &'static str, error: bool, cx: &mut Context<Self>) {
        let text = self.state.lang.tr(key);
        self.state.begin_compact_command(id);
        self.state.finish_compact_command(id, text.clone(), error);
        self.state.notice(text);
        cx.notify();
    }

    /// The typed `/compact` line: the command takes NO arguments.
    ///
    /// The shipped handler refuses a non-empty trimmed input with its usage
    /// line and `kind: "error"`; a bare invocation is the ordinary command.
    /// Routing it here means `/compact` is a command even while the slash card
    /// is closed, instead of being sent to the model as prose.
    pub(crate) fn compact_command_line(&mut self, args: &str, cx: &mut Context<Self>) {
        if !args.trim().is_empty() {
            let Some(id) = self.state.active_id.clone() else {
                return;
            };
            self.refuse_compact(&id, "slash.compactUsage", true, cx);
            return;
        }
        self.compact_history(cx);
    }

    fn clear_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.composer
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.composer_value.clear();
        cx.notify();
    }

    /// Spawn a turn whose transcript and budget the state has already opened.
    ///
    /// The composer, the scheduler and the queue all reach a turn this way, so
    /// the agent they run is built and accounted for identically.
    pub(crate) fn spawn_turn(
        &mut self,
        id: String,
        input: react::llm::Content,
        transcript: Vec<crate::model::ChatMessage>,
        budget: usize,
        cx: &mut Context<Self>,
    ) {
        // The vault is the ONLY source for a provider credential: the entry
        // (or provider:<id>:api_key) the session's selected provider names is
        // resolved here, at build time, so the secret never has to be written
        // into the settings file.
        let choice = self.state.session_choice(&id);
        // No turn opens against a model nobody chose. The composer gates this
        // before it opens a turn; this is the same gate for the queue and the
        // scheduler, whose session may have lost its model in between.
        if let Some(key) = self.state.settings.refusal_for(&choice) {
            let why = crate::i18n::tr(self.state.lang, key);
            self.state
                .apply_event(&id, crate::runner::AgentEvent::Error(why.clone()));
            self.state.notice(why);
            return;
        }
        let built = crate::caps::build_agent_for_session_with_subagents_and_jobs(
            &self.state.settings,
            &self.state.vault,
            &choice,
            &self.state.mcp,
            &id,
            &id,
            self.state.subagents.clone(),
            self.state.jobs.clone(),
        );
        if let Some(error) = &built.credential_error {
            // No turn opens without a credential: a locked vault names itself
            // instead of sending the request unauthenticated.
            self.state.notice(error.message(self.state.lang));
            return;
        }
        self.state.set_tools_schema(built.tools_schema_chars);
        self.state.set_policy(built.policy.clone());
        let agent = built.agent;
        // The agent shares this store with its `update_plan` tool, so the
        // to-do panel reads the running plan through it.
        self.state.attach_plan(&id, agent.plan());
        // The queue dock steers into this turn through the agent's own
        // mailbox. It is attached here, at the one turn-opening path, and
        // dropped when the turn ends - so only a live turn can be steered.
        self.state.attach_steers(&id, agent.steers().clone());
        let cap = self.state.effective_in(&id).tool_output_cap;
        // The automatic trigger prices the request the turn is about to send:
        // the history window plus the rows `begin_turn_in` just appended (the
        // user prompt and its empty assistant placeholder). Above the workspace
        // ceiling the runner condenses the oldest balanced span before the turn
        // streams, which is the reference's pressure listener.
        let auto_retain = {
            // The two rows begin_turn_in appended, borrowed rather than cloned:
            // this runs on every send.
            let appended: &[crate::model::ChatMessage] = self
                .state
                .store
                .session(&id)
                .map(|session| &session.messages[session.messages.len().saturating_sub(2)..])
                .unwrap_or(&[]);
            self.state
                .auto_compaction_retain(&id, &transcript, appended, budget)
        };
        let rx = self
            .state
            .runner
            .spawn(&id, agent, input, transcript, auto_retain, cap);
        self.pump(id, rx, cx);
    }

    /// Save the queue row being edited, which is Enter in its own editor.
    fn save_queue_edit(&mut self, cx: &mut Context<Self>) {
        let Some(seq) = self.queue_edit_for else {
            return;
        };
        let text = self.queue_edit.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        if let Some(id) = self.state.active_id.clone() {
            self.state.edit_queued(&id, seq, text);
        }
        self.queue_edit_for = None;
        cx.notify();
    }

    /// Start the next queued message once a session's turn has ended.
    ///
    /// The reference drains its inbox as each turn finishes, so a reader who
    /// submits while the agent runs gets a second turn rather than a second
    /// stream writing over the same transcript.
    pub(crate) fn drain_inbox(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.state.is_streaming(id) {
            return;
        }
        let Some(queued) = self.state.pop_inbox(id) else {
            return;
        };
        let plan = self.state.plan_mode(id);
        let request = crate::state::turn_text(&queued.text, plan);
        let input = crate::state::turn_input(&request, &queued.attachments);
        let display = crate::state::display_text(&queued.text, &queued.attachments);
        let Some((transcript, budget)) = self.state.begin_turn_in(id, &display) else {
            return;
        };
        self.spawn_turn(id.to_string(), input, transcript, budget, cx);
    }

    /// Start every automation task that has come due.
    ///
    /// The reference runs a task in the session it was created in, so its events
    /// fold into that session and the reader sees the turn when they open it. The
    /// tasks come from the SAVED settings, which is what the pane own Save
    /// writes; a session that is gone switches its task off rather than firing
    /// into nothing, and a task whose session is already streaming is skipped for
    /// this tick.
    pub(crate) fn check_schedules(&mut self, cx: &mut Context<Self>) {
        let now = crate::settings::unix_now();
        let due = self.state.settings.global.due_schedules(now);
        for id in due {
            let Some(task) = self
                .state
                .settings
                .global
                .schedules
                .iter()
                .find(|entry| entry.id == id)
                .cloned()
            else {
                continue;
            };
            if self.state.streams.contains_key(&task.session_id) {
                continue;
            }
            let Some((transcript, budget)) =
                self.state.begin_turn_in(&task.session_id, &task.prompt)
            else {
                self.state.settings.global.toggle_schedule(&id);
                continue;
            };
            let plan = self.state.plan_mode(&task.session_id);
            let request = crate::state::turn_text(&task.prompt, plan);
            let input = crate::state::turn_input(&request, &[]);
            self.spawn_turn(task.session_id.clone(), input, transcript, budget, cx);
            self.state.settings.global.mark_schedule_ran(&id, now);
        }
        cx.notify();
    }

    /// Forward runner events into the state until the turn ends.
    fn pump(&self, id: String, rx: async_channel::Receiver<AgentEvent>, cx: &mut Context<Self>) {
        // Keep the elapsed-time line moving while the turn streams.
        cx.spawn(async move |this, cx| loop {
            let live = this
                .update(cx, |this, cx| {
                    if this.state.any_streaming() {
                        cx.notify();
                        true
                    } else {
                        false
                    }
                })
                .unwrap_or(false);
            if !live {
                break;
            }
            cx.background_executor()
                .timer(std::time::Duration::from_millis(500))
                .await;
        })
        .detach();

        cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let done = matches!(&event, AgentEvent::Done);
                let failed = matches!(&event, AgentEvent::Error(_));
                let terminal = done || failed;
                let session_id = id.clone();
                let _ = this.update(cx, |shell, cx| {
                    shell.state.apply_event(&session_id, event);
                    cx.notify();
                });
                if terminal {
                    // The turn is over, so its plan is the session's own now and
                    // whatever waited behind it can start.
                    let _ = this.update(cx, |shell, cx| {
                        shell.state.finish_plan(&session_id);
                        shell.state.persist_subagents();
                        // A finished turn can append trajectory rows, so a page
                        // the trace had exhausted is worth asking for again.
                        shell.trace_done = false;
                        shell.drain_inbox(&session_id, cx);
                        // An armed goal keeps going: a completed round starts the
                        // next one, and a failed round disarms rather than looping.
                        if failed {
                            shell.disarm_goal_on_error(&session_id, cx);
                        } else {
                            shell.continue_goal(&session_id, cx);
                        }
                    });
                    break;
                }
            }
            // A stream that ends without a terminal event (a stop) still has to
            // release its store, or the saved plan would never catch up, and the
            // queue behind it still belongs to the reader.
            let _ = this.update(cx, |shell, cx| {
                shell.state.finish_plan(&id);
                shell.state.persist_subagents();
                shell.trace_done = false;
                shell.drain_inbox(&id, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// The reference's chat content width for the column measured on this
    /// frame, `clamp(680px, conversation-column * .64, 920px)` over a
    /// conversation column of the viewport less the sidebar. See
    /// `crate::theme::chat_width_at`.
    pub(crate) fn chat_w(&self) -> f32 {
        crate::theme::resolve_content_width(self.conversation_w.as_f32(), self.chat_user_w)
    }

    /// The composer card's width for this frame.
    ///
    /// The card is the chat width plus the measured 32, but never more than the
    /// seat the conversation column actually has - the reference's
    /// `width:100%;max-width:chat + 32` inside a 16px-a-side seat. Every
    /// consumer of the card (the stack, the queue dock, the to-do panel, the
    /// hero) derives from this, so they cannot disagree about the narrow case.
    pub(crate) fn composer_w(&self) -> f32 {
        crate::theme::composer_width_at(self.conversation_w.as_f32(), self.chat_w())
    }

    /// The hero stack's width: the card plus its clearance a side, capped at
    /// the column the way the reference's `min(card + 2 * clearance, 100%)` is.
    pub(crate) fn hero_w(&self) -> f32 {
        crate::theme::hero_width_at(self.conversation_w.as_f32(), self.composer_w())
    }

    /// Ask for the one-time legacy compaction, off the UI thread.
    ///
    /// A history is checked once per session selection: opening it is the cheap
    /// manifest path, and only a log that predates the index (or carries a
    /// scattered tail) is rewritten. The rewrite builds a sibling directory and
    /// swaps it by rename, so no frame waits on it.
    pub(crate) fn maybe_compact_history(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.state.active_id.clone() else {
            return;
        };
        if !self.compaction_checked.insert(id.clone()) {
            return;
        }
        if !self.state.store.history_needs_compaction(&id) {
            return;
        }
        let Some(dir) = self.state.store.history_dir(&id) else {
            return;
        };
        let target = dir.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { crate::history::compact_directory(&target) })
                .await;
            let _ = this.update(cx, |this, cx| match result {
                // Another window finished the rewrite first. The cached index
                // still names the files its swap removed, so it is dropped
                // just the same, without a repaint to announce nothing new.
                Ok(crate::history::Compaction::Noop) => {
                    this.state.store.forget_history(&dir);
                }
                Ok(crate::history::Compaction::Rewritten { .. }) => {
                    // The swap moved the whole directory, so the cached index
                    // points at files that are gone: the next page read must
                    // open the sealed one.
                    this.state.store.forget_history(&dir);
                    cx.notify();
                }
                // Busy: another compactor owns the marker and the live
                // directory is still readable, so the cached index stays.
                Ok(crate::history::Compaction::Busy) | Err(_) => {}
            });
        })
        .detach();
    }

    /// Ask for the first trajectory page, or the next older one.
    ///
    /// The trace tab reads its tool rows from the Parquet log rather than from
    /// the messages in memory, so a long run is scrollable without being
    /// resident. The index snapshot is taken here; the decode runs on the
    /// background executor and comes back through the entity. A page read ahead
    /// is applied without touching the disk, and a request while one is loading
    /// is the same request, so it is dropped.
    ///
    /// Returns whether a read started or a buffered page was applied.
    pub(crate) fn request_trace_page(&mut self, cx: &mut Context<Self>) -> bool {
        if self.trace_done {
            return false;
        }
        let Some(id) = self.state.active_id.clone() else {
            return false;
        };
        let page = self.trace_pages;
        if self.trace.is_busy() {
            return false;
        }
        let key = PageKey {
            session: id.clone(),
            key: page,
        };
        if let Some(pending) = self
            .trace
            .take_ahead(|pending| pending.session == id && pending.page == page)
        {
            return self.land_trace(pending, cx);
        }
        self.trace.clear_ahead();
        let Some(read) = self.state.begin_trace_page(&id, page) else {
            self.trace_done = true;
            return false;
        };
        if !self.trace.begin(key) {
            return false;
        }
        cx.spawn(async move |this, cx| {
            let page = cx
                .background_executor()
                .spawn(async move { read.read() })
                .await;
            let key = PageKey {
                session: page.session.clone(),
                key: page.page,
            };
            let _ = this.update(cx, |this, cx| {
                if this.trace.finish(&key) {
                    this.land_trace(page, cx);
                }
            });
        })
        .detach();
        true
    }

    /// Apply one decoded trajectory page above the loaded one.
    fn land_trace(&mut self, page: TracePage, cx: &mut Context<Self>) -> bool {
        if self.state.active_id.as_deref() != Some(page.session.as_str())
            || page.page != self.trace_pages
        {
            return false;
        }
        if page.rows.is_empty() {
            self.trace_done = true;
            return false;
        }
        let added = page.rows.len();
        // Each page is OLDER than the last, so it is prepended: the trace reads
        // oldest to newest. The newest rows fall off the bottom once the loaded
        // window is several pages wide, so a deep scroll cannot grow without
        // bound; they are still in the Parquet log.
        let mut rows = page.rows;
        rows.extend(std::mem::take(&mut self.trace_rows));
        const TRACE_MAX_ROWS: usize = 4 * crate::history::PAGE;
        rows.truncate(TRACE_MAX_ROWS);
        self.trace_rows = rows;
        self.trace_pages += 1;
        self.trace_session = self.state.active_id.clone();
        self.start_trace_ahead(cx);
        cx.notify();
        added > 0
    }

    /// Read the next trajectory page ahead of the reader.
    ///
    /// At most one is in flight and one is buffered; a log that has no page
    /// left latches the flow done so a reader parked at the top does not ask
    /// again on every frame.
    pub(crate) fn start_trace_ahead(&mut self, cx: &mut Context<Self>) {
        if self.trace_done || self.trace.has_ahead() {
            return;
        }
        let Some(id) = self.state.active_id.clone() else {
            return;
        };
        let page = self.trace_pages;
        let Some(read) = self.state.begin_trace_page(&id, page) else {
            self.trace_done = true;
            return;
        };
        let key = PageKey {
            session: id,
            key: page,
        };
        if !self.trace.begin_ahead(key) {
            return;
        }
        cx.spawn(async move |this, cx| {
            let page = cx
                .background_executor()
                .spawn(async move { read.read() })
                .await;
            let key = PageKey {
                session: page.session.clone(),
                key: page.page,
            };
            let _ = this.update(cx, |this, _cx| {
                if this.trace.finish_ahead(&key) {
                    this.trace.store_ahead(page);
                }
            });
        })
        .detach();
    }

    /// Jump to a turn from the rail.
    ///
    /// The newest mark is the way back to the live end: it re-engages the tail, so
    /// the chat follows again from anywhere. Every other mark leaves the tail,
    /// because a jump up the transcript must not be undone by the next frame.
    pub(crate) fn jump_to_turn(&mut self, child: usize, newest: bool) {
        if newest {
            self.following_tail = true;
            self.transcript_scroll.scroll_to_bottom();
        } else {
            self.following_tail = false;
            self.transcript_scroll.scroll_to_top_of_item(child);
        }
    }

    /// Page in the window of messages above the transcript when the reader
    /// reaches its top.
    ///
    /// This is the memory half of the paged history: a session keeps only the
    /// last windows of its Parquet log in memory, and the older ones arrive as
    /// the reader scrolls to them. The read is SCHEDULED here, never performed
    /// here, so a slow disk cannot stall the frame. The anchor is applied on a
    /// later frame, so the message they were looking at stays where it was.
    pub(crate) fn page_older_at_the_top(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some(anchor) = self.older_anchor.take() {
            self.transcript_scroll.scroll_to_top_of_item(anchor);
            cx.notify();
            return false;
        }
        // Both logs are paged per session: a session switch starts them over.
        // The in-flight page, if any, lands into the flow of the session that
        // asked for it and is dropped when the flow no longer names it.
        if self.state.active_id != self.trace_session {
            self.trace_session = self.state.active_id.clone();
            self.trace_pages = 0;
            self.trace_rows.clear();
            self.trace_done = false;
            self.trace.reset();
            self.older.reset();
        }
        if self.state.tab == crate::state::Tab::Trajectory {
            // The trace has its own paging: reaching its top pulls the next page
            // of the trajectory log, and the rows arrive above what is drawn.
            let at_top = f32::from(self.traj_scroll.offset().y) > -crate::theme::PAGE_TRIGGER;
            return at_top && self.request_trace_page(cx);
        }
        if self.following_tail {
            return false;
        }
        // Near the top: the scroller's offset is zero at the oldest painted
        // row and negative going down.
        if self.chat_last_offset < -(crate::theme::PAGE_TRIGGER) {
            return false;
        }
        let at_start = self
            .state
            .active_session()
            .map(|session| session.paged_from == Some(0))
            .unwrap_or(true);
        if at_start {
            return false;
        }
        self.request_older_page(cx)
    }

    /// Ask for the window of older messages above the transcript.
    ///
    /// A page already read ahead is applied without touching the disk; a page
    /// still loading coalesces this ask; otherwise the read is scheduled on the
    /// background executor. The index snapshot is O(1) and the rows come back
    /// through the entity, so the render path never blocks on the files.
    pub(crate) fn request_older_page(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(read) = self.state.begin_older_read() else {
            return false;
        };
        if self.older.is_busy() {
            return false;
        }
        let key = PageKey {
            session: read.session().to_string(),
            key: read.from(),
        };
        if let Some(pending) = self
            .older
            .take_ahead(|pending| pending.session == key.session && pending.from == key.key)
        {
            return self.land_older(pending, cx);
        }
        self.older.clear_ahead();
        self.start_older_read(read, key, cx);
        true
    }

    /// Schedule one decoded window above the loaded chat.
    fn start_older_read(
        &mut self,
        read: crate::store::OlderRead,
        key: PageKey,
        cx: &mut Context<Self>,
    ) {
        if !self.older.begin(key) {
            return;
        }
        cx.spawn(async move |this, cx| {
            let page = cx
                .background_executor()
                .spawn(async move { read.read() })
                .await;
            let key = PageKey {
                session: page.session.clone(),
                key: page.from,
            };
            let _ = this.update(cx, |this, cx| {
                if this.older.finish(&key) {
                    this.land_older(page, cx);
                }
            });
        })
        .detach();
    }

    /// Apply one decoded window above the loaded chat and keep the reader's
    /// place.
    fn land_older(&mut self, page: OlderPage, cx: &mut Context<Self>) -> bool {
        if self.state.active_id.as_deref() != Some(page.session.as_str()) {
            return false;
        }
        let added = self
            .state
            .finish_older_read(page, crate::theme::WINDOWS_KEPT);
        if added == 0 {
            return false;
        }
        if self.state.tab == crate::state::Tab::Trajectory {
            self.traj_select = None;
            self.traj_scroll.scroll_to_top_of_item(0);
        } else {
            // The anchor is the row the reader was looking at: it becomes the
            // top of the viewport once the older rows are above it.
            self.older_anchor = Some(added.saturating_sub(1));
        }
        self.start_older_ahead(cx);
        cx.notify();
        true
    }

    /// Read the window before the loaded one, ahead of the reader.
    ///
    /// At most one is in flight and one is buffered, and a log at its first row
    /// has nothing to ask for. This is what makes the next scroll instant
    /// without turning every reach into an eager whole-log load.
    pub(crate) fn start_older_ahead(&mut self, cx: &mut Context<Self>) {
        if self.older.has_ahead() {
            return;
        }
        let Some(read) = self.state.begin_older_read() else {
            return;
        };
        let key = PageKey {
            session: read.session().to_string(),
            key: read.from(),
        };
        if !self.older.begin_ahead(key) {
            return;
        }
        cx.spawn(async move |this, cx| {
            let page = cx
                .background_executor()
                .spawn(async move { read.read() })
                .await;
            let key = PageKey {
                session: page.session.clone(),
                key: page.from,
            };
            let _ = this.update(cx, |this, _cx| {
                if this.older.finish_ahead(&key) {
                    this.older.store_ahead(page);
                }
            });
        })
        .detach();
    }

    /// The left inset the transcript keeps, narrower than the right band.
    ///
    /// The chat is not centred: the right edge stays where the turn rail and
    /// the composer expect it, and the left edge moves out, because the empty
    /// strip beside the text was the widest thing on a wide window.
    pub(crate) fn chat_left_inset(&self) -> f32 {
        crate::theme::chat_left_inset(self.conversation_w.as_f32(), self.chat_w())
    }

    /// The measured width-handle strip for the live column.
    pub(crate) fn handle_w(&self) -> f32 {
        crate::theme::handle_width_at(self.conversation_w.as_f32(), self.chat_w())
    }

    /// A pointer landed on a width handle: remember the side, the x it started
    /// at and the width the column had then - origin and base in the reference.
    pub(crate) fn begin_chat_drag(
        &mut self,
        side: HandleSide,
        pointer_x: f32,
        cx: &mut Context<Self>,
    ) {
        self.chat_drag = Some(ChatDrag {
            side,
            origin_x: pointer_x,
            base: self.chat_w(),
            moved: false,
        });
        cx.notify();
    }

    /// The pointer moved with a drag in flight. The reference's outwardWidth is
    /// base + outward * 2, where outward is the dx signed toward the dragged
    /// side, because the content column stays centred while one edge moves.
    pub(crate) fn drag_chat_width(&mut self, pointer_x: f32, cx: &mut Context<Self>) {
        let Some(drag) = self.chat_drag else { return };
        let dx = pointer_x - drag.origin_x;
        let outward = match drag.side {
            HandleSide::Right => dx,
            HandleSide::Left => -dx,
        };
        if dx != 0.0 {
            self.chat_drag = Some(ChatDrag {
                moved: true,
                ..drag
            });
        }
        let width = drag.base + outward * 2.0;
        self.chat_user_w = Some(crate::theme::resolve_content_width(
            self.conversation_w.as_f32(),
            Some(width),
        ));
        cx.notify();
    }

    /// The pointer lifted. The reference commits only when the pointer actually
    /// moved, and stores the RESOLVED width, under the measured client-store key
    /// dsh.conversation.contentWidth; bsh's store is the settings file.
    pub(crate) fn commit_chat_width(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.chat_drag.take() else {
            return;
        };
        if drag.moved {
            self.state.draft_mut().global.general.content_width = self.chat_user_w;
            // The width was just written to the draft, so persisting `settings`
            // would store the copy from before the drag. The commit is what
            // moves the draft into the document and writes it.
            let _ = self.state.commit_settings();
            // The drag kept the previous width's row heights so it could stay
            // cheap; now that the width is settled, measure them exactly once.
            self.transcript_layout.get_mut().request_remeasure();
        }
        cx.notify();
    }

    /// The pointer entered a width handle: the reference reveals the bar on
    /// :hover. The strip stops the move's propagation, so the body's own move
    /// (which clears this) is what makes leaving the strip hide it again.
    pub(crate) fn hover_chat_handle(&mut self, side: HandleSide, cx: &mut Context<Self>) {
        if self.chat_hover != Some(side) {
            self.chat_hover = Some(side);
            cx.notify();
        }
    }

    /// The pointer moved somewhere the handles are not, ending the reference's
    /// :hover. A drag keeps the bar up on its own, through chat_drag.
    pub(crate) fn clear_chat_hover(&mut self, cx: &mut Context<Self>) {
        if self.chat_hover.take().is_some() {
            cx.notify();
        }
    }

    /// The reference's resolved sidebar column: the dragged width, or its own
    /// 280 store default, inside the measured 264..420 contract.
    pub(crate) fn sidebar_w(&self) -> f32 {
        crate::theme::resolve_sidebar_width(self.sidebar_user_w)
    }

    /// The reference's `pressKill`: the first press arms the button, the
    /// confirming press stops the job. bsh's stop is local (the runner's stop
    /// plus the stream removal), so the only failure it can observe is a job that
    /// is already gone - and the reference's failed phase is the same shape, a
    /// hint held for its measured window.
    pub(crate) fn press_job_kill_at(
        &mut self,
        id: &str,
        at: std::time::Instant,
        cx: &mut Context<Self>,
    ) {
        let armed = self.jobs_arm.as_ref().is_some_and(|arm| {
            arm.id == id && arm.phase == super::header::JobPhase::Armed && !arm.expired(at)
        });
        if armed {
            self.jobs_arm = None;
            // The stop is the roster's own kill: it marks the job and signals
            // the whole process group. A job that already settled reports
            // false, which is the reference's failed phase.
            if !self.state.jobs.kill(id) {
                self.jobs_arm = Some(super::header::JobArm {
                    id: id.to_string(),
                    phase: super::header::JobPhase::Failed,
                    at,
                });
            }
        } else {
            self.jobs_arm = Some(super::header::JobArm {
                id: id.to_string(),
                phase: super::header::JobPhase::Armed,
                at,
            });
        }
        cx.notify();
    }

    /// Drop an arming or failure hint that has outlived its measured window,
    /// reporting whether one went.
    pub(crate) fn expire_job_arm(&mut self, at: std::time::Instant) -> bool {
        if self.jobs_arm.as_ref().is_some_and(|arm| arm.expired(at)) {
            self.jobs_arm = None;
            return true;
        }
        false
    }

    /// One tick of the jobs panel's own clock: it re-renders the elapsed figures
    /// and drops an expired kill hint. The reference runs this once a second
    /// while the popover is open over a live row; the kit popover carries no
    /// open-change seam here, so bsh ticks while a background job exists at all -
    /// which is the only state in which the trigger, and so the popover, can be
    /// drawn.
    pub(crate) fn tick_jobs(&mut self, at: std::time::Instant, cx: &mut Context<Self>) {
        // The roster's version is what makes a settlement repaint once. A
        // running job also needs the tick for its elapsed figure; a settled
        // roster with nothing expanded is idle-cheap.
        let version = self.state.jobs.version();
        let live = self.state.has_running_jobs();
        let changed = version != self.jobs_counts_version;
        if changed {
            self.refresh_jobs_snapshot();
            self.prune_job_output_cache();
        }
        let subagents = self
            .state
            .subagent_runs()
            .iter()
            .any(|run| run.status.is_running());
        if self.state.subagent.is_some() {
            self.state.refresh_subagent_preview();
        }
        self.state.persist_subagents();
        let expired = self.expire_job_arm(at);
        if live || changed || subagents || expired {
            cx.notify();
        }
    }

    /// Resolve the chip counts once per roster version or session switch, so
    /// the header does not walk the job roster on every frame.
    pub(crate) fn refresh_jobs_snapshot(&mut self) {
        let version = self.state.jobs.version();
        let active = self.state.active_id.clone();
        if version == self.jobs_counts_version && active == self.jobs_counts_for {
            return;
        }
        self.jobs_counts = self.state.job_counts();
        self.jobs_counts_for = active;
        self.jobs_counts_version = version;
    }

    /// Re-read the output of every expanded row, rebuilding a cached string
    /// only when its ring moved. The popover calls this once per frame; a
    /// static row compares two integers and copies nothing.
    pub(crate) fn refresh_job_output_cache(&mut self) {
        if !self.jobs_open || self.jobs_expanded.is_empty() {
            self.jobs_output_cache.clear();
            return;
        }
        let ids: Vec<String> = self.jobs_expanded.iter().cloned().collect();
        for id in ids {
            match self.state.jobs.output_state(&id) {
                Some((bytes, dropped)) => {
                    let stale = self
                        .jobs_output_cache
                        .get(&id)
                        .is_none_or(|(held, gone, _, _)| *held != bytes || *gone != dropped);
                    if stale {
                        if let Some(output) = self.state.jobs.output_for(&id) {
                            self.jobs_output_cache.insert(
                                id,
                                (
                                    output.bytes,
                                    output.dropped,
                                    output.text,
                                    output.dropped > 0,
                                ),
                            );
                        }
                    }
                }
                None => {
                    self.jobs_output_cache.remove(&id);
                }
            }
        }
        let expanded = self.jobs_expanded.clone();
        self.jobs_output_cache.retain(|id, _| expanded.contains(id));
    }

    /// The exact text the expanded row draws, or None when it is not expanded.
    pub(crate) fn job_output_cached(&self, id: &str) -> Option<(&str, bool)> {
        self.jobs_output_cache
            .get(id)
            .map(|(_, _, text, truncated)| (text.as_str(), *truncated))
    }

    /// Drop cache entries for jobs the active session no longer lists.
    fn prune_job_output_cache(&mut self) {
        let live: std::collections::HashSet<String> = self
            .state
            .active_id
            .as_deref()
            .map(|id| {
                self.state
                    .jobs
                    .list_for_session(id)
                    .into_iter()
                    .map(|job| job.id)
                    .collect()
            })
            .unwrap_or_default();
        self.jobs_expanded.retain(|id| live.contains(id));
        let expanded = self.jobs_expanded.clone();
        self.jobs_output_cache
            .retain(|id, _| live.contains(id) && expanded.contains(id));
    }

    /// The reference's own fold resolution: below the 1024 breakpoint the
    /// column folds unless the reader expanded it over the squeezed centre.
    pub(crate) fn sidebar_folded(&self) -> bool {
        crate::theme::sidebar_folded(
            self.viewport_w,
            self.state.sidebar_collapsed,
            self.state.narrow_expanded,
        )
    }

    /// The reference's own toggleSidebar: a narrow viewport flips the runtime
    /// narrowExpanded flag so the column can expand over the centre, and only a
    /// wide one flips the stored collapse.
    pub(crate) fn toggle_sidebar_at(&mut self, viewport: f32, cx: &mut Context<Self>) {
        if viewport < crate::theme::SIDEBAR_AUTO_COLLAPSE {
            self.state.narrow_expanded = !self.state.narrow_expanded;
        } else {
            self.state.sidebar_collapsed = !self.state.sidebar_collapsed;
        }
        cx.notify();
    }

    /// A mousedown on the sidebar's own handle: the reference records the
    /// column width and the pointer x and then works from that pair.
    pub(crate) fn begin_sidebar_drag(&mut self, pointer_x: f32, cx: &mut Context<Self>) {
        self.sidebar_drag = Some(SideDrag {
            origin_x: pointer_x,
            base: self.sidebar_w(),
            moved: false,
        });
        cx.notify();
    }

    /// The reference's own onSidebarDrag: setSidebar(base + dx), clamped by
    /// clampWidth, applied live and stored once the pointer is released.
    pub(crate) fn drag_sidebar_width(&mut self, pointer_x: f32, cx: &mut Context<Self>) {
        let Some(drag) = self.sidebar_drag.as_mut() else {
            return;
        };
        drag.moved = true;
        let width =
            crate::theme::resolve_sidebar_width(Some(drag.base + (pointer_x - drag.origin_x)));
        if self.sidebar_user_w != Some(width) {
            self.sidebar_user_w = Some(width);
            cx.notify();
        }
    }

    /// A pointerup ends the resize with the reference's final dx applied, and
    /// only a real move is worth a store write.
    pub(crate) fn commit_sidebar_width(&mut self, pointer_x: f32, cx: &mut Context<Self>) {
        let Some(drag) = self.sidebar_drag.take() else {
            return;
        };
        let width =
            crate::theme::resolve_sidebar_width(Some(drag.base + (pointer_x - drag.origin_x)));
        if drag.moved {
            self.sidebar_user_w = Some(width);
            self.state.draft_mut().global.general.sidebar_width = self.sidebar_user_w;
            // Same as the content width: the value lives in the draft now, so
            // only a commit writes it where the next launch reads it.
            let _ = self.state.commit_settings();
            // A wider or narrower sidebar changes the conversation column and
            // with it the transcript's width, so the rows are measured once
            // here, after the drag rather than on every pointer move.
            self.transcript_layout.get_mut().request_remeasure();
        }
        cx.notify();
    }

    /// Stop the active session's stream.
    pub(crate) fn stop_active(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.state.active_id.clone() {
            self.state.stop(&id);
            cx.notify();
        }
    }

    /// Put a session's title into the rename field, in place on its row.
    ///
    /// The reference renames inline, so the row the reader pointed at is where
    /// the field appears; nothing opens over the conversation.
    pub(crate) fn begin_session_rename(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(title) = self.state.session_title(id) else {
            return;
        };
        self.renaming_session = Some(id.to_string());
        let field = self.session_rename.clone();
        field.update(cx, |state, cx| state.set_value(title, window, cx));
        // Focus AFTER this frame: when the menu item was the door, the menu's own
        // close handed focus back to whatever held it and our field blurred in the
        // same dispatch, so the rename vanished as it appeared.
        window.defer(cx, move |window, cx| {
            field.update(cx, |state, cx| {
                state.focus_handle(cx).focus(window, cx);
            });
        });
        cx.notify();
    }

    /// Apply the rename; a blank field leaves the title alone.
    /// Ease the transcript towards its wheel target, one frame at a time.
    ///
    /// Returns whether another frame is needed. The target is set by
    /// [`Shell::wheel_chat`], so a wheel event only decides WHERE to go; the
    /// motion happens here, which is what makes it smooth instead of a jump.
    pub(crate) fn advance_smooth(&mut self) -> bool {
        let mut animating = false;
        if self.chat_smooth.animating() {
            let current = f32::from(self.transcript_scroll.offset().y);
            if let Some(next) = self.chat_smooth.step(current) {
                self.transcript_scroll.set_offset(point(px(0.0), px(next)));
                animating = self.chat_smooth.animating();
            }
        }
        if self.traj_smooth.animating() {
            let current = f32::from(self.traj_scroll.offset().y);
            if let Some(next) = self.traj_smooth.step(current) {
                self.traj_scroll.set_offset(point(px(0.0), px(next)));
                animating = animating || self.traj_smooth.animating();
            }
        }
        animating
    }

    /// Take a wheel delta meant for the chat transcript.
    ///
    /// Two things happen, and both matter. The tail-follow is switched OFF: the
    /// shell re-pins the transcript to the newest message every frame while it is
    /// on, so a reader could not scroll up at all until they used the rail. And the
    /// delta becomes a TARGET the frame loop eases towards, instead of an instant
    /// jump.
    pub(crate) fn wheel_chat(&mut self, delta: f32, cx: &mut Context<Self>) {
        if delta == 0.0 {
            return;
        }
        self.following_tail = false;
        // `jumped` is what the scroller itself did before this listener ran; taking
        // it back leaves the easing as the only thing that moves the transcript.
        let jumped = self.take_back_chat_jump();
        let step = if jumped != 0.0 { jumped } else { delta };
        let max = f32::from(self.transcript_scroll.max_offset().y);
        self.chat_smooth.aim(super::smooth::wheel_target(
            self.chat_last_offset,
            step,
            max,
        ));
        cx.notify();
    }

    /// Take a wheel delta meant for the trajectory tab.
    pub(crate) fn wheel_trajectory(&mut self, delta: f32, cx: &mut Context<Self>) {
        if delta == 0.0 {
            return;
        }
        let current = f32::from(self.traj_scroll.offset().y);
        let jumped = current - self.traj_last_offset;
        if jumped.abs() > 0.5 {
            self.traj_scroll
                .set_offset(point(px(0.0), px(self.traj_last_offset)));
        }
        let step = if jumped.abs() > 0.5 { jumped } else { delta };
        let max = f32::from(self.traj_scroll.max_offset().y);
        self.traj_smooth.aim(super::smooth::wheel_target(
            self.traj_last_offset,
            step,
            max,
        ));
        cx.notify();
    }

    /// Undo whatever the scroller did on its own; returns how far it jumped.
    fn take_back_chat_jump(&mut self) -> f32 {
        let current = f32::from(self.transcript_scroll.offset().y);
        let jumped = current - self.chat_last_offset;
        if jumped.abs() > 0.5 {
            self.transcript_scroll
                .set_offset(point(px(0.0), px(self.chat_last_offset)));
        }
        jumped
    }

    /// The chat target for a delta, clamped to the scroller's range.
    /// Open the workspace row's title for editing.
    pub(crate) fn begin_workspace_rename(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self
            .state
            .settings
            .global
            .workspaces
            .iter()
            .find(|workspace| workspace.id == id)
            .map(|workspace| workspace.name.clone())
        else {
            return;
        };
        self.renaming_workspace = Some(id.to_string());
        let field = self.workspace_rename.clone();
        field.update(cx, |state, cx| state.set_value(name, window, cx));
        // Focus after this frame, for the reason the session field documents:
        // the menu closes in the same dispatch and hands focus back first.
        window.defer(cx, move |window, cx| {
            field.update(cx, |state, cx| {
                state.focus_handle(cx).focus(window, cx);
            });
        });
        cx.notify();
    }

    /// Commit the workspace rename, if the field is open.
    pub(crate) fn commit_workspace_rename(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.renaming_workspace.take() else {
            return;
        };
        let name = self.workspace_rename.read(cx).value().to_string();
        if !self.state.rename_workspace(&id, &name) {
            // A blank name is not a rename; keep the field open rather than
            // silently discarding the edit.
            self.renaming_workspace = Some(id);
        }
        cx.notify();
    }

    /// Remove a workspace from the list, rehoming its chats.
    pub(crate) fn delete_workspace(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.state.delete_workspace(id) {
            // The file pane's paths belonged to the deleted root.
            self.refile_workspace(cx);
        }
        cx.notify();
    }

    /// Delete a chat, out of the menu that asked for it.
    ///
    /// The row goes, the file goes with it (a list row leaving must not let a
    /// transcript come back on the next launch), and any rename field on that
    /// row is put away first so it cannot commit into a chat that no longer
    /// exists.
    pub(crate) fn delete_session(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.renaming_session.as_deref() == Some(id) {
            self.renaming_session = None;
        }
        self.state.delete_session(id);
        cx.notify();
    }

    /// Open a chat in a named workspace from its own row.
    pub(crate) fn new_session_in_workspace(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(session) = self.state.new_session_in(id) {
            self.state.select(&session);
        }
        self.refile_workspace(cx);
        cx.notify();
    }

    pub(crate) fn commit_session_rename(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.renaming_session.take() else {
            return;
        };
        let title = self.session_rename.read(cx).value().to_string();
        self.state.rename_session(&id, &title);
        cx.notify();
    }

    /// Drop the rename without touching the title.
    pub(crate) fn cancel_session_rename(&mut self, cx: &mut Context<Self>) {
        if self.renaming_session.take().is_some() {
            cx.notify();
        }
    }

    /// Branch a session and open the copy, which is what Fork session means.
    pub(crate) fn fork_session_at(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(forked) = self.state.fork_session(id) else {
            return;
        };
        // The fork carries the same rows at the same geometry, so it inherits the
        // source's measured heights instead of re-measuring a long chat.
        self.transcript_layout.get_mut().adopt(id, &forked);
        self.state.select(&forked);
        self.maybe_compact_history(cx);
        self.renaming_session = None;
        cx.notify();
    }

    /// Copy one message's text and mark that row as copied for a moment.
    ///
    /// The clipboard write is the whole action; the check mark is the
    /// reference's one-second confirmation, and it is cleared by a timer rather
    /// than by the next repaint so a still frame keeps showing it.
    pub(crate) fn copy_message(&mut self, index: usize, text: &str, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        self.copied_message = Some(index);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPY_FLASH).await;
            let _ = this.update(cx, |this, cx| {
                if this.copied_message == Some(index) {
                    this.copied_message = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Branch the conversation at one assistant message and open the branch.
    ///
    /// The new session holds the transcript up to that message, which is the
    /// reference's `fork({ atSeq })`. The composer takes focus because a branch
    /// exists to be continued from where it was cut.
    pub(crate) fn branch_into_session(
        &mut self,
        id: &str,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(forked) = self.state.branch_session_at(id, index) else {
            return;
        };
        // The branch renders the rows the reader is already looking at, so its
        // measured heights are the source's. Adopting them here keeps the first
        // frame after the click a PAINT: without it the new session is a cache
        // miss and every row of a long chat is rebuilt, which is the stall this
        // button used to pay.
        self.transcript_layout.get_mut().adopt(id, &forked);
        self.state.select(&forked);
        self.composer
            .update(cx, |input, cx| input.focus(window, cx));
        self.maybe_compact_history(cx);
        self.renaming_session = None;
        self.rail_hover = None;
        cx.notify();
    }

    /// The parsed body for one text, created once and reused while it matches.
    ///
    /// The identity is (kind, hash of the text): two bodies with identical text
    /// genuinely render identically, so sharing one parse costs nothing and
    /// saves a second one. The stored text is compared on a hit, so a hash
    /// collision only costs a reparse, never a wrong body.
    pub(crate) fn body_state(
        &self,
        kind: u8,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Entity<TextViewState> {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        let key = (kind, hasher.finish());
        let frame = self.frame_no;
        let mut states = self.body_states.borrow_mut();
        if let Some((snapshot, used, state)) = states.get_mut(&key) {
            if snapshot != text {
                snapshot.clear();
                snapshot.push_str(text);
                let next = snapshot.clone();
                // The streaming path: a body that grew keeps its parsed
                // document and appends only the new tail.
                state.update(cx, |state, cx| state.set_text(&next, cx));
            }
            *used = frame;
            return state.clone();
        }
        #[cfg(test)]
        BODIES_PARSED.with(|parsed| parsed.set(parsed.get() + 1));
        let state = cx.new(|cx| TextViewState::markdown(text, cx));
        states.insert(key, (text.to_string(), frame, state.clone()));
        // Bound the cache: drop the body that has gone longest without a paint.
        if states.len() > BODY_STATES_MAX {
            let oldest = states
                .iter()
                .min_by_key(|(_, (_, used, _))| *used)
                .map(|(key, _)| *key);
            if let Some(oldest) = oldest {
                states.remove(&oldest);
            }
        }
        state
    }
    /// Take the window's current text selection to the clipboard, if a drag
    /// left one.
    ///
    /// This is copy-on-selection: the reference is a web page, where selecting a
    /// reply and pressing the platform copy key is the reader's own gesture. A
    /// native text view gets the same result by copying when the drag ends, so
    /// the text is on the clipboard without a second keystroke. A selection that
    /// is empty (a click on plain text, a click on a control) writes nothing.
    pub(crate) fn copy_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let text = gpui_kit::base::TextSelection::selected_text(window, cx)
            .trim()
            .to_string();
        if text.is_empty() {
            return false;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }
    /// Put a session's id on the clipboard, which is what a bug report needs.
    pub(crate) fn copy_session_id(&mut self, id: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(id.to_string()));
        cx.notify();
    }
    /// Select a session and focus the composer.
    pub(crate) fn open_session(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        // A rename belongs to the row it was opened on; moving to another
        // session puts it away rather than leaving a field on an unselected row.
        if self.renaming_session.as_deref() != Some(id) {
            self.cancel_session_rename(cx);
        }
        self.state.select(id);
        self.maybe_compact_history(cx);
        self.composer
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    /// Create a chat and focus the composer.
    pub(crate) fn new_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.new_session();
        self.composer
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    /// The message index of every user turn, in order.
    pub(crate) fn turn_indices(&self) -> Vec<usize> {
        self.state
            .active_session()
            .map(|session| {
                session
                    .messages
                    .iter()
                    .enumerate()
                    // A steering row belongs to the turn it interjected into,
                    // so it is not a mark of its own.
                    .filter(|(_, message)| {
                        message.role == crate::model::Role::User && !message.steering
                    })
                    .map(|(index, _)| index)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Keep the active mark in the rail's view unless the pointer is on it.
    ///
    /// `TurnNavigatorRail` scrolls to the active index only when it changes and
    /// skips the follow while the pointer is inside the frame, so a reader
    /// choosing a mark is never fighting the scroll.
    pub(crate) fn follow_rail(&mut self) {
        let turns = self.turn_indices();
        let active = self.visible_turn(&turns);
        if active == self.rail_active {
            return;
        }
        self.rail_active = active;
        if self.rail_hover.is_none() {
            // The handle counts the scroller's children, i.e. marks.
            if let Some(index) = active.and_then(|turn| turns.iter().position(|item| *item == turn))
            {
                self.rail_scroll.scroll_to_item(index);
            }
        }
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The markdown bodies' cache ages by frame: every body this frame paints
        // is stamped, and the least recently painted is evicted when the cache
        // fills.
        self.frame_no = self.frame_no.wrapping_add(1);
        // The chip counts are cached per roster version, and the expanded job
        // rows read their output here: an unchanged ring costs two integers
        // instead of a 64 KiB copy.
        self.refresh_jobs_snapshot();
        self.refresh_job_output_cache();
        // The rail's container is the conversation column, which is the window
        // less the sidebar (`@container (width <= 900px)` hides the frame).
        // The frame's width is what the reference's auto-collapse measures, so
        // record it before the fold is resolved. The height caps the slash
        // palette's list, so it is recorded from the same measurement.
        self.viewport_w = window.viewport_size().width.as_f32();
        self.viewport_h = window.viewport_size().height.as_f32();
        // The composer's placeholder names the current mode: plan mode asks for
        // a task to plan, a blank session asks what to build, a conversation
        // asks for the next message. It is fitted to the field's own width, so
        // a narrow card ellipsises the text instead of cutting it; both the
        // phase and the fitted width are cached, because `set_placeholder`
        // always notifies and must not run per frame.
        let placeholder = match self.claim_hint() {
            Some(claim) => claim,
            // The reference's own order: a steerable queue outranks plan mode,
            // because the key is one press away from acting on it.
            None if self.queued_steerable() => "composer.placeholderSteerQueue",
            None if self.state.active_plan_mode() => "composer.planPlaceholder",
            None if self.state.hero_phase() => "composer.heroPlaceholder",
            None => "composer.placeholder",
        };
        // The key interceptor has no window, so a Tab completion hands its text
        // here and the frame that owns a window writes it. This is the same
        // pattern the hero placeholder uses.
        if let Some(text) = self.slash.pending.take() {
            self.set_composer_text(&text, window, cx);
        }
        let sidebar = if self.sidebar_folded() {
            px(0.0)
        } else {
            px(self.sidebar_w())
        };
        // The conversation column is what the centre column actually gets, so
        // an open right panel is part of the arithmetic: the panel is a sibling
        // column in the same row, and measuring without it made the chat width,
        // the width handles and the rail's own container query believe in space
        // the layout had already given away.
        let panel = if self.right.open {
            px(self.right_panel_w())
        } else {
            px(0.0)
        };
        self.conversation_w = window.viewport_size().width - sidebar - panel;
        // The fit is derived from THIS frame's column, so it lands after the
        // measurement and before the first child reads `chat_w`. The phase, the
        // field width and the language are one cache key: the text is shaped,
        // and `set_placeholder` always notifies.
        let field_w = crate::theme::composer_field_width(self.composer_w());
        let fit = (field_w.to_bits(), self.state.lang);
        if placeholder != self.placeholder_key || self.placeholder_fit != Some(fit) {
            self.placeholder_key = placeholder;
            self.placeholder_fit = Some(fit);
            let size =
                crate::theme::body_size_at(self.state.draft().global.general.content_font_size);
            let text = self.state.lang.tr(placeholder);
            let fitted = super::composer::fit_placeholder(window, size, &text, field_w);
            self.composer
                .update(cx, |input, cx| input.set_placeholder(fitted, window, cx));
        }
        // The rail follows the active mark, unless the pointer is working it.
        self.follow_rail();
        // Follow the tail, the state the reference marks as data-chat-following-tail:
        // the newest message stays in view until the reader leaves the end. A rail
        // jump clears the flag; content growing while the reader is still at the end
        // (or a fresh session) sets it again, so a reader who scrolled up is left alone.
        let active = self.state.active_id.clone();
        if active != self.tail_session {
            self.tail_session = active;
            self.tail_max = None;
            self.following_tail = true;
            self.think_open.clear();
            self.compaction_open.clear();
            // The audit's folds name turn and call positions in the transcript
            // that just changed, so a switch closes them and starts the list at
            // the top the way the reference resets a mounted pane.
            self.audit_turn_open.clear();
            self.audit_open.clear();
            self.audit_full.clear();
            self.audit_query.clear();
            self.audit_jump_to = None;
            self.audit_search
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.rail_hover = None;
            self.rail_active = None;
            self.changes_open.clear();
            self.changes_hover = None;
            self.changes_preview = None;
            self.changes_cache.borrow_mut().clear();
            // A turn list names a message index in the session that just went
            // away, so it goes with it; a file comparison names a workspace
            // file and is still meaningful, which is why only the turn clears.
            self.right.diff.turn = None;
            self.state.stat_open = false;
            self.state.meter_open = false;
            // The jobs popover is scoped to the session it was opened in, so a
            // switch closes it and drops the expanded rows with its output.
            self.jobs_open = false;
            self.jobs_expanded.clear();
            self.jobs_output_cache.clear();
        }
        // A pending audit jump lands here, after the Chat scroller has mounted
        // for one frame. Setting it from the Audit tab was lost with the
        // unmounted element; asking on this frame gives prepaint real bounds.
        if self.state.tab == crate::state::Tab::Chat {
            match self.audit_jump_to {
                Some((child, 0)) => {
                    self.audit_jump_to = None;
                    self.transcript_scroll.scroll_to_top_of_item(child);
                }
                Some((child, wait)) => self.audit_jump_to = Some((child, wait - 1)),
                None => {}
            }
        }
        let scroll = &self.transcript_scroll;
        let offset = scroll.offset().y;
        let max = scroll.max_offset().y;
        let was_at_end = self
            .tail_max
            .is_none_or(|previous| previous + offset <= px(TAIL_EPSILON));
        if was_at_end && self.tail_max.is_some_and(|previous| max > previous) {
            self.following_tail = true;
        }
        if self.following_tail {
            scroll.scroll_to_bottom();
            // The first frame on a new session lays the content out but reports
            // no height yet, so one follow-up frame is what turns the measured
            // height into an offset. That request is bounded by the measurement
            // being MISSING: asking again while the height is still zero (an
            // empty transcript, or the hero phase) repainted an idle shell at
            // the display's frame rate forever.
            if max <= px(0.0) && self.tail_max.is_none() {
                cx.notify();
            }
        }
        self.tail_max = Some(max);
        // The wheel's easing runs after the tail decision, and asks for the next
        // frame while it is still moving.
        if self.advance_smooth() {
            cx.notify();
        }
        self.chat_last_offset = f32::from(self.transcript_scroll.offset().y);
        self.traj_last_offset = f32::from(self.traj_scroll.offset().y);
        self.page_older_at_the_top(cx);
        // A rail whose marks overflow keeps the NEWEST mark in view: the reader
        // is at the tail, and the last mark is the one they press to come back.
        if self.following_tail {
            // max_offset is a MAGNITUDE (positive), while the live offset runs
            // from zero down to its negative: the rail is only at the bottom when
            // the two agree in size.
            let max = self.rail_scroll.max_offset().y;
            if max > px(0.0) && self.rail_scroll.offset().y > -max {
                self.rail_scroll.scroll_to_bottom();
            }
        }
        let overlay = if self.state.overlay == Overlay::Settings {
            // The panel is sized against the viewport, like the reference's
            // own min(800px, 100vh - 48px) height.
            let viewport = window.viewport_size();
            Some(self.render_settings(viewport.width.as_f32(), viewport.height.as_f32(), cx))
        } else {
            None
        };
        // The delete confirmation sits over the settings dialog: the reference
        // draws a dialog there rather than deleting on the row control.
        let confirm_delete =
            if self.state.overlay == Overlay::Settings && self.task_delete.is_some() {
                Some(self.render_task_delete(cx))
            } else {
                None
            };
        // The full-access risk gate is reachable from the General pane's row and
        // from the composer chip, so it is hosted here rather than inside either.
        let trash = self.trash_open.then(|| self.render_trash(cx));
        let confirm_permission = if self.permission_confirm.is_some() {
            Some(self.render_permission_confirm(cx))
        } else {
            None
        };
        // The provider delete confirmation: the card it guards stays in the
        // list and the reference's own dialog is drawn over the panel.
        let confirm_provider_delete =
            if self.state.overlay == Overlay::Settings && self.state.removing_provider.is_some() {
                Some(self.render_provider_delete(cx))
            } else {
                None
            };
        // The trust confirmation: an installed package's commands are shown
        // and approved here, over the panel that listed them.
        let confirm_trust =
            if self.state.overlay == Overlay::Settings && self.trust_confirm.is_some() {
                Some(self.render_trust_confirm(cx))
            } else {
                None
            };
        // The vault's own delete confirmation, drawn over the same panel.
        let confirm_vault_delete =
            if self.state.overlay == Overlay::Settings && self.vault_confirm_delete.is_some() {
                Some(self.render_vault_delete(cx))
            } else {
                None
            };
        div()
            .id("shell-frame")
            .flex()
            .flex_row()
            .size_full()
            .overflow_hidden()
            .relative()
            // The reference's DragHandle captures its pointer, so its own
            // onPointerUp ends the drag; gpui hands the release back to the
            // pressed strip but the MOVES to whatever is under the pointer,
            // so the frame carries those. It is a no-op without a drag.
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                // A seam only follows a HELD pointer. The strip is 8px wide, so a
                // horizontal drag leaves it at once; the release then went to
                // whatever was under the pointer and the strip's own mouse-up
                // never ran, leaving the drag armed - the panel kept resizing
                // after the button came up. Reading the button off the move is
                // the state a drag cannot drift out of.
                if event.pressed_button.is_none() && this.right.drag.take().is_some() {
                    cx.notify();
                }
                this.drag_sidebar_width(event.position.x.as_f32(), cx);
                this.drag_right_panel_width(event.position.x.as_f32(), cx);
            }))
            // The reference dismisses its jobs popover on a press outside it.
            // The trigger and the panel stop their own presses, so this only
            // sees a click that landed elsewhere.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    if this.jobs_open {
                        this.jobs_open = false;
                        cx.notify();
                    }
                }),
            )
            // The session menu's accelerators, handled at the frame so they work
            // from wherever focus sits inside it - the composer, a field, a list
            // row. They act on the session the reader has open.
            .on_action(
                cx.listener(|this, _: &super::session_menu::RenameSession, window, cx| {
                    if let Some(id) = this.state.active_id.clone() {
                        this.begin_session_rename(&id, window, cx);
                    }
                }),
            )
            .on_action(
                cx.listener(|this, _: &super::session_menu::ForkSession, _window, cx| {
                    if let Some(id) = this.state.active_id.clone() {
                        this.fork_session_at(&id, cx);
                    }
                }),
            )
            .on_action(cx.listener(
                |this, _: &super::session_menu::ArchiveSession, _window, cx| {
                    if let Some(id) = this.state.active_id.clone() {
                        this.state.archive_session(&id);
                        cx.notify();
                    }
                },
            ))
            .bg(rgb(crate::theme::page()))
            .text_color(rgb(crate::theme::text()))
            .when(!self.sidebar_folded(), |root| {
                root.child(self.render_sidebar(cx))
            })
            // A maximised panel over a folded sidebar consumes the whole row:
            // the main column gets zero width and draws nothing, and at that
            // size it still LAYS ITS HEADER OUT across the panel's band (its
            // folded 76px left pad alone makes the header 104px wide), which is
            // an overlap the corner test pins. A zero-width column is not a
            // column, so it is not rendered at all; closing the panel brings it
            // back through the ordinary path and it owns the corner again.
            .when(!self.right_panel_owns_the_corner(), |root| {
                root.child(self.render_main_column(cx))
            })
            .when_some(self.render_right_panel(cx), |root, panel| root.child(panel))
            // The jobs popover is painted after every column so it can overlap
            // the transcript and the right panel; it is opened from the header
            // chip and closes on an outside press (below) or a session switch.
            .when_some(self.render_jobs_panel(cx), |root, panel| root.child(panel))
            // The reference draws its DragHandle over the seam at z-index 11,
            // and only while the column is expanded.
            .when(!self.sidebar_folded(), |root| {
                root.child(self.render_sidebar_handle(cx))
            })
            .when_some(overlay, |root, overlay| root.child(overlay))
            .when_some(confirm_delete, |root, confirm| root.child(confirm))
            .when_some(confirm_permission, |root, confirm| root.child(confirm))
            .when_some(confirm_provider_delete, |root, confirm| root.child(confirm))
            .when_some(confirm_trust, |root, confirm| root.child(confirm))
            .when_some(confirm_vault_delete, |root, confirm| root.child(confirm))
            .when_some(trash, |root, panel| root.child(panel))
    }
}

#[cfg(test)]
mod tests {
    // Named import: a glob would pull in gpui's re-export of the test attribute
    // macro, which shadows the builtin one its own expansion emits.
    use super::selection_drag;

    /// Copy-on-selection copies on a DRAG and not on a click.
    #[test]
    fn a_click_is_not_a_selection_drag() {
        assert!(!selection_drag(0.0, 0.0), "a still press is a click");
        assert!(!selection_drag(1.0, 1.0), "and a wobbling one still is");
        assert!(selection_drag(4.0, 0.0), "travelling sideways is a drag");
        assert!(selection_drag(0.0, 4.0), "and so is travelling down");
        assert!(selection_drag(300.0, 0.0), "a real selection always counts");
    }
}
