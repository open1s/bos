//! The right panel: Workspace Files, the managed terminal, the diff surface
//! and the browser, behind one tab strip.
//!
//! The panel is the reference's own right column: a tab strip that carries one
//! chip per open pane, a header of controls on its right edge, and either the
//! Start screen's cards or the body of the pane the reader chose. Only
//! Workspace Files is built here so far; the terminal and the browser take the
//! same tab, so what they need is a tab chip and a body, not a second frame.
//!
//! Workspace Files is a browser, not a tree: one directory is listed at a time,
//! directories lead files, and choosing a directory replaces the listing. That
//! is the reference's own shape, and it is also the one that survives a
//! five-thousand-file workspace without a virtual list.
//!
//! Everything the pane reads and writes goes through [`crate::files`], which
//! confines every path to the workspace root. The panel never joins a path
//! itself.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::diff::{Diff, DiffEvent, DiffFile, DiffMode, DiffState};
use gpui_kit::component::input::{
    Editor, EditorState, Input, InputEvent, InputState, RangeDecoration, RangeDecorationCollection,
    RangeDecorationStyle, Textarea, TextareaState,
};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::text::{TextView, TextViewState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::Focusable;
use gpui_kit::*;

use super::diff_pane::{self, DiffSource};
use super::federation_pane::{FederationActivity, FederationPreview};
use super::shell::{Shell, SideDrag};
use super::terminal_view::TerminalView;
use super::{hairline_bottom, hairline_left, handle_bar, icon, txt, DivExt, IconName};
use crate::browser::engine::cdp::PointerButton;
use crate::files::{self, Kind};
use crate::model::Role;
use crate::terminal::KeyOutcome;
use crate::theme;
use gpui_libghostty::TerminalOptions;

// Save the file the panel is showing, bound to cmd-s.
//
// The editor surface carries no save button - the panel's body is the
// editor, and a bar repeating the tab chip's name and one control read as
// chrome - so the keyboard shortcut is the whole affordance, the way a
// native document editor does it.
actions!(
    bsh_files,
    [SaveFile, TerminalTab, BrowserTab, FocusLauncher, AddComment]
);

/// How close in time two blank-space presses must be to read as a double-click.
///
/// macOS tracks its own double-click interval, but the harness's synthetic
/// clicks carry no timing, so the pane measures it: a second press inside this
/// window, and inside [`SELF_DOUBLE_PX`] of the first, is a double-click.
const SELF_DOUBLE_MS: u64 = 400;

/// How close in space two blank-space presses must be to read as a double-click.
///
/// Roughly the width of a fingertip's drift between two presses of the same
/// spot; a press on the far side of the blank area is a different intention.
const SELF_DOUBLE_PX: f32 = 6.0;

/// The top-left a document-anchored overlay sits at, in the editor's own
/// coordinates.
///
/// The kit reports text boxes in WINDOW space while `absolute()` places a child
/// against its PARENT, so subtracting the element's own viewport origin is the
/// whole conversion. Two traps live here: using `last_bounds` instead of
/// `input_bounds` folds the editor's SCROLL into the origin (a scrolled document
/// puts every overlay hundreds of pixels below its selection), and the offset was
/// once added twice (once by the kit, once by the panel to the window's right),
/// which is why the note affordance only showed while the editor happened to
/// start at the window's left edge.
///
/// The point is the middle of the part of `bounds` the document SHOWS: the
/// control sits on the text the reader marked rather than hanging off one
/// edge, and it is pulled back inside the editor so a selection on the last
/// line cannot push it out of the document.
fn document_point(bounds: Bounds<Pixels>, editor: Bounds<Pixels>, size: f32) -> (f32, f32) {
    // A long line runs past the right edge, so the horizontal middle is taken
    // from the slice the editor actually shows; the vertical middle is the
    // line's own, which `anchor_line` has already picked from the visible rows.
    let visible = bounds.intersect(&editor);
    let center_x = if f32::from(visible.size.width) > 0.0 {
        visible.center().x
    } else {
        bounds.center().x
    };
    let x = f32::from(center_x - editor.origin.x) - size / 2.0;
    let y = f32::from(bounds.center().y - editor.origin.y) - size / 2.0;
    let max_x = (f32::from(editor.size.width) - size - 4.0).max(4.0);
    let max_y = (f32::from(editor.size.height) - size - 4.0).max(4.0);
    (x.clamp(4.0, max_x), y.clamp(4.0, max_y))
}

/// The line the note affordance rides: the middle of the part of the selection
/// the viewport shows, as a one-based line number, or `None` when the marks are
/// entirely outside the laid-out rows.
///
/// The whole-range box cannot be the answer. The kit lays out only the visible
/// lines, so a selection whose first or last line has scrolled out arrives
/// with a degenerate box - which made the + vanish the moment the reader
/// scrolled, even while most of the marked text was still on screen. Picking a
/// line that IS laid out and asking for that line's own slice keeps the
/// control on the document: it follows the selection through the scroll, and it
/// goes only when the marks do.
pub(crate) fn anchor_line(
    text: &str,
    range: &std::ops::Range<usize>,
    rows: &std::ops::Range<usize>,
) -> Option<usize> {
    if range.start >= range.end || rows.start >= rows.end {
        return None;
    }
    let first = crate::notes::line_at_offset(text, range.start);
    // The end of a selection that stops at a line break points at the next
    // line, which would name a row the reader never marked.
    let last = crate::notes::line_at_offset(text, range.end.saturating_sub(1));
    // `rows` is zero-based and half-open; the line numbers are one-based.
    let visible_first = rows.start + 1;
    let visible_last = rows.end;
    let from = first.max(visible_first);
    let to = last.min(visible_last);
    if from > to {
        return None;
    }
    Some(from + (to - from) / 2)
}

/// [`document_point`]'s conversion for an overlay that keeps the measured
/// box's own top-left instead of centring on it - a note card hangs UNDER its
/// anchor line, so it wants the line's bottom edge, not its middle.
fn document_origin(bounds: Bounds<Pixels>, editor: Bounds<Pixels>) -> (f32, f32) {
    (
        f32::from(bounds.origin.x - editor.origin.x),
        f32::from(bounds.origin.y - editor.origin.y),
    )
}

/// The editor element's own box, which is the overlay container's frame.
///
/// The kit keeps two boxes: `input_bounds` is the element's viewport (what the
/// reader sees, scroll NOT folded in) and `last_bounds` is the same box after
/// the editor's scroll offset moved it - negative once the document is scrolled,
/// which is exactly how a scrolled mark ended up hundreds of pixels below the
/// selection. The overlays are children of the element, so the viewport box is
/// the one every window-space text box is converted against.
fn viewport_box(state: &EditorState) -> Bounds<Pixels> {
    state.input_bounds()
}

/// How much room the review card needs under its anchor.
///
/// The card has no fixed height (a quote of up to three lines, a growing field
/// and one action row), so the flip decision reserves the tallest it can get
/// rather than measuring a surface that has not laid out yet. A card that does
/// not fit below its lines opens above them, which is the shape a context menu
/// uses for the same reason.
const CARD_ROOM: f32 = 260.0;

/// The wording the panel reports an I/O failure with.
///
/// The reference prints its own `fileReferences` error text beside the row
/// that failed; bsh has no remote to quote, so it prints what the operating
/// system said, which is the more useful half of the same idea.
fn io_msg(error: &std::io::Error) -> String {
    error.to_string()
}

/// The editing meaning of one keystroke, for the input gate's mirror.
///
/// Only what changes the line the reader is typing is mirrored: printable text,
/// Backspace, and the line editor's kill chords. Arrows, history recall and
/// completion are deliberately absent - they are not gated, and what they do to
/// the line is the shell's business, not the policy's.
fn terminal_edit(keystroke: &Keystroke) -> Option<crate::terminal::Edit> {
    use crate::terminal::Edit;
    if keystroke.modifiers.platform {
        return None;
    }
    let key = keystroke.key.as_str();
    if keystroke.modifiers.control {
        return match key {
            "u" | "c" => Some(Edit::KillLine),
            "w" | "backspace" => Some(Edit::KillWord),
            _ => None,
        };
    }
    match key {
        "backspace" => Some(Edit::Backspace),
        // Enter is the submit itself, and the other editing keys move the
        // cursor rather than the line.
        "enter" | "tab" | "escape" | "up" | "down" | "left" | "right" | "home" | "end"
        | "pageup" | "pagedown" | "insert" | "delete" => None,
        "space" => Some(Edit::Text(String::from(" "))),
        _ => {
            let text = keystroke.key_char.as_deref()?;
            // A composition or a dead key can deliver more than one character;
            // a control character is not text the line editor adds.
            let printable = text
                .chars()
                .all(|ch| !ch.is_control() && ch != '\n' && ch != '\r');
            printable.then(|| Edit::Text(text.to_string()))
        }
    }
}

/// Whether a keystroke is the Enter the input gate decides.
///
/// A chord with the platform modifier belongs to the application, so it is not
/// the shell's Enter; a Ctrl or Alt Enter is a different key for a terminal
/// program, not a line submit.
fn terminal_is_enter(keystroke: &Keystroke) -> bool {
    keystroke.key == "enter"
        && !keystroke.modifiers.platform
        && !keystroke.modifiers.control
        && !keystroke.modifiers.alt
}

impl gpui::EntityInputHandler for Shell {
    fn text_for_range(
        &mut self,
        _range: std::ops::Range<usize>,
        _adjusted_range: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::UTF16Selection> {
        Some(gpui::UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    /// A committed composition is the only text either surface should see; the
    /// marked preview belongs to the IME's own candidate window.
    ///
    /// Which surface receives it is decided by FOCUS. The page's handler exists
    /// for two reasons: it routes non-printable commands (Backspace, arrows,
    /// Delete) back to the key listener, which AppKit otherwise swallows, and
    /// it receives the text of a printable key. Because the key listener ALSO
    /// types that character (a real keydown plus char, so the page sees the
    /// key), the echo is recognised and dropped - MEASURED: without that,
    /// `rust` became `ruusstt`. A commit that DIFFERS from the just-typed key
    /// supersedes it (one Backspace, then the commit), which is the route a
    /// synthetic Unicode keystroke and a real IME conversion take.
    fn replace_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if text.is_empty() {
            return;
        }
        if self.browser_focus.is_focused(window) {
            if let Some(engine) = self.state.browser.engine() {
                let recent = self
                    .browser_typed
                    .take()
                    .filter(|(at, _)| at.elapsed() <= std::time::Duration::from_millis(300));
                if let Some((_, typed)) = recent {
                    // The echo of what the key listener already inserted, in
                    // full: one character for a printable key, the whole
                    // string for a composition that arrives as a key too.
                    if typed == text {
                        cx.notify();
                        return;
                    }
                    // A longer commit SUPERSEDES a just-typed key, which is
                    // the shape of a synthetic Unicode keystroke and of an
                    // IME conversion: the character the key inserted is not
                    // part of what the commit means. Only a one-character key
                    // is removed, because that is the only case where the
                    // stray character is known to be one character long.
                    if typed.chars().count() == 1 {
                        let _ = engine.press("backspace", true);
                    }
                }
                let _ = engine.insert_text(text, true);
            }
            cx.notify();
            return;
        }
        // A committed composition is text, never a line submit: an IME does not
        // press Enter, so the gate is not asked here, and a newline a commit
        // happens to carry is dropped rather than run un-gated.
        let text = text.replace(['\r', '\n'], "");
        if let Some(terminal) = self.right.terminal.as_mut() {
            terminal.track(crate::terminal::Edit::Text(text.clone()));
        }
        if let Some(view) = self.right.terminal_view.clone() {
            view.update(cx, |view, _| view.commit_text(&text));
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        _new_text: &str,
        _new_selected_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn bounds_for_range(
        &mut self,
        _range: std::ops::Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(element_bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(0)
    }
}

/// Which pane the right panel is showing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RightTab {
    /// The workspace file browser and editor.
    Files,
    /// The managed terminal: a real interactive shell on its own PTY, parsed
    /// and drawn by libghostty.
    Terminal,
    /// The comparison surface: a patch, a conflicted file, or a file on its
    /// own, drawn in unified or split. It is the one pane whose VIEW the reader
    /// switches from the strip, so its two controls sit in the top bar.
    Diff,
    /// The builtin browser. Not built yet; see [`RightTab::Terminal`].
    Browser,
    /// Extension-contributed declarative panels.
    Extensions,
    /// The federation node's peers and grants.
    Federation,
    /// Durable tasks: running, queued, scheduled and settled.
    Tasks,
}

impl RightTab {
    /// Parse the CLI spelling of a pane, for the launch deep link.
    pub(crate) fn from_cli(name: &str) -> Option<Self> {
        match name {
            "files" => Some(Self::Files),
            "terminal" => Some(Self::Terminal),
            "diff" => Some(Self::Diff),
            "browser" => Some(Self::Browser),
            "extensions" | "plugins" => Some(Self::Extensions),
            "federation" => Some(Self::Federation),
            "tasks" => Some(Self::Tasks),
            _ => None,
        }
    }

    /// The tab's label key in the string table.
    pub(crate) fn key(self) -> &'static str {
        match self {
            RightTab::Files => "right.tab.files",
            RightTab::Terminal => "right.tab.terminal",
            RightTab::Diff => "right.tab.diff",
            RightTab::Browser => "right.tab.browser",
            RightTab::Extensions => "right.tab.extensions",
            RightTab::Federation => "right.tab.federation",
            RightTab::Tasks => "right.tab.tasks",
        }
    }

    /// The tab's leading glyph.
    pub(crate) fn glyph(self) -> IconName {
        match self {
            RightTab::Files => IconName::FolderOpen,
            RightTab::Terminal => IconName::SquareTerminal,
            RightTab::Diff => IconName::GitCompareArrows,
            RightTab::Browser => IconName::Globe,
            RightTab::Extensions => IconName::Puzzle,
            RightTab::Federation => IconName::GitBranch,
            RightTab::Tasks => IconName::ClipboardList,
        }
    }

    /// The one-line description the Workspace Launcher shows under the card.
    pub(crate) fn description_key(self) -> &'static str {
        match self {
            RightTab::Files => "right.start.filesHint",
            RightTab::Terminal => "right.start.terminalHint",
            RightTab::Diff => "right.start.diffHint",
            RightTab::Browser => "right.start.browserHint",
            RightTab::Extensions => "right.start.extensionsHint",
            RightTab::Federation => "right.start.federationHint",
            RightTab::Tasks => "right.start.tasksHint",
        }
    }

    /// The pane's stable id stem, for the strip's controls and the tests that
    /// press them. The label key is dotted ("right.tab.diff"), which makes a
    /// poor element id, so the two are separate facts.
    pub(crate) fn slug(self) -> &'static str {
        match self {
            RightTab::Files => "files",
            RightTab::Terminal => "terminal",
            RightTab::Diff => "diff",
            RightTab::Browser => "browser",
            RightTab::Extensions => "extensions",
            RightTab::Federation => "federation",
            RightTab::Tasks => "tasks",
        }
    }

    /// The pane with this slug, for a persisted launcher row.
    pub(crate) fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tab| tab.slug() == slug)
    }

    /// Every pane the panel can show, in the order the Start screen lists the
    /// ones it offers.
    const ALL: [RightTab; 7] = [
        RightTab::Files,
        RightTab::Diff,
        RightTab::Terminal,
        RightTab::Browser,
        RightTab::Extensions,
        RightTab::Federation,
        RightTab::Tasks,
    ];

    /// Whether the Start screen offers this pane as a card.
    ///
    /// Diff is the one pane it does not: a comparison needs a file, and the
    /// surface has no picker of its own, so a card could only open it empty.
    /// The pane is reached where its material is - a file row's Diff control,
    /// a step's View diff - and from the strip, which still offers every pane
    /// because the panel can show them all. Keeping Diff in [`Self::ALL`] is
    /// what lets both facts share one ordered list.
    pub(crate) fn on_start_screen(self) -> bool {
        !matches!(self, RightTab::Diff)
    }
}

/// The right panel's own state: whether it is open, which pane is up, and the
/// width the reader dragged out.
#[derive(Default)]
pub(crate) struct RightPanel {
    /// Whether the panel is on screen.
    pub(crate) open: bool,
    /// The pane showing, or none for the Start screen.
    pub(crate) tab: Option<RightTab>,
    /// The panes the reader has opened, in open order.
    ///
    /// This is the tab set, not a cache of shortcuts: every pane in it stays
    /// behind its own tab until that tab is closed, so switching from Browser to
    /// Files and back returns to the same surfaces rather than reopening them.
    pub(crate) open_panes: Vec<RightTab>,
    /// The width a reader dragged out, before it is committed.
    pub(crate) user_w: Option<f32>,
    /// Whether the panel has taken the whole column, which is what its header's
    /// maximise control does.
    pub(crate) maximized: bool,
    /// Whether the launcher's recent list is showing every row.
    pub(crate) launcher_recent_open: bool,
    /// The resize in flight, if any.
    pub(crate) drag: Option<SideDrag>,
    /// The managed terminal, spawned when the Terminal tab is first opened.
    pub(crate) terminal: Option<crate::terminal::Terminal>,
    /// The native libghostty surface the terminal pane draws, created with
    /// the terminal and dropped with it.
    pub(crate) terminal_view: Option<Entity<super::terminal_view::TerminalView>>,
    /// The focus subscriptions that tell the surface whether the pane owns the
    /// keyboard; replaced when the surface is replaced.
    pub(crate) terminal_view_subs: Vec<Subscription>,
    /// The comparison surface's own state: what it is showing, and how.
    pub(crate) diff: super::diff_pane::DiffView,
    /// The federation pane's peers and drafts.
    pub(crate) federation: FederationPane,
    /// The Tasks board's records, read once per open so a frame never touches
    /// the store.
    pub(crate) tasks: std::cell::RefCell<Option<Vec<crate::extension::tasks::Task>>>,
    /// Task rows whose detail is expanded.
    pub(crate) expanded: std::collections::BTreeSet<String>,
    /// Whether the History section shows its rows.
    pub(crate) history_open: bool,
    /// Whether the graph fits itself to the pane (true) or keeps a manual zoom.
    pub(crate) graph_fit: bool,
    /// Whether Finder's Show All Tabs catalogue covers the body.
    ///
    /// The strip's overflow menu catalogs what did not fit; this is the
    /// deliberate overview of every tab, reached from the tab context menu the
    /// way Finder reaches View > Show All Tabs.
    pub(crate) catalog_open: bool,
}

/// One enrolled peer as the federation pane lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FederationPeerRow {
    /// The Node id, full length.
    pub(crate) node: String,
    /// The name the peer gave when it was enrolled, if any.
    pub(crate) name: Option<String>,
    /// The trust state: pending, trusted, or revoked.
    pub(crate) state: String,
    /// The capabilities the peer may invoke.
    pub(crate) grants: Vec<String>,
}

/// One delegation as the federation pane lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FederationDelegationRow {
    /// The delegation id.
    pub(crate) id: String,
    /// The target Node.
    pub(crate) target: String,
    /// The bounded objective.
    pub(crate) objective: String,
    /// The state label key.
    pub(crate) state: &'static str,
    /// The failure reason, when there is one.
    pub(crate) error: Option<String>,
    /// Whether the delegation can still be cancelled.
    pub(crate) cancellable: bool,
}

/// One resource lease as the federation pane lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FederationLeaseRow {
    /// The lease id.
    pub(crate) id: String,
    /// The capability the lease grants access to.
    pub(crate) capability: String,
    /// The holder Node.
    pub(crate) holder: String,
    /// Bytes used.
    pub(crate) used: u64,
    /// Byte ceiling, when bounded.
    pub(crate) max: Option<u64>,
    /// Expiry, when bounded.
    pub(crate) expires_at_ms: Option<u64>,
}

/// One artifact file in the local federation store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FederationArtifactRow {
    /// Display name.
    pub(crate) name: String,
    /// Size in bytes.
    pub(crate) bytes: u64,
    /// Absolute path, for removal.
    pub(crate) path: std::path::PathBuf,
}

/// One locally served knowledge item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FederationKnowledgeRow {
    /// The item id.
    pub(crate) id: String,
    /// Its class label.
    pub(crate) class: String,
    /// The provenance source.
    pub(crate) source: String,
}

/// The trust-state filter the peers card applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PeerFilter {
    /// Every enrolled peer.
    #[default]
    All,
    /// Peers this node trusts.
    Trusted,
    /// Peers whose grants were withdrawn.
    Revoked,
    /// Peers whose enrollment is still waiting on a decision.
    Pending,
}

/// One peer seen on the announce key, with the capabilities it advertises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscoveredPeerRow {
    /// The Node id.
    pub(crate) node: String,
    /// The name the peer announced, if any.
    pub(crate) name: Option<String>,
    /// Advertised capability ids and their risk classes.
    pub(crate) capabilities: Vec<(String, String)>,
}
/// The federation pane's own state.
#[derive(Default)]
pub(crate) struct FederationPane {
    /// The peers, in store order.
    pub(crate) peers: Vec<FederationPeerRow>,
    /// The peer the grant card applies to.
    pub(crate) selected: Option<usize>,
    /// Whether the list has been read since the pane opened.
    pub(crate) loaded: bool,
    /// The last failure, shown above the list.
    pub(crate) error: Option<String>,
    /// The last success, shown above the list.
    pub(crate) notice: Option<String>,
    /// The peer a revoke is waiting on a second press to confirm.
    pub(crate) confirm_revoke: Option<String>,
    /// The peer a removal is waiting on a second press to confirm.
    pub(crate) confirm_remove: Option<String>,
    /// The operation a pending removal owns.
    pub(crate) confirm_remove_op: Option<u64>,
    /// Whether the bulk "remove revoked" is waiting on a second press.
    pub(crate) confirm_remove_revoked: bool,
    /// The operation a pending bulk removal owns.
    pub(crate) confirm_remove_revoked_op: Option<u64>,
    /// Whether removing every peer is waiting on a second press.
    pub(crate) confirm_remove_all: bool,
    /// The operation a pending remove-all owns.
    pub(crate) confirm_remove_all_op: Option<u64>,
    /// Peers seen on the announce key, excluding this node.
    pub(crate) discovered: Vec<DiscoveredPeerRow>,
    /// Delegation records, newest first.
    pub(crate) delegations: Vec<FederationDelegationRow>,
    /// Resource leases this Node granted.
    pub(crate) leases: Vec<FederationLeaseRow>,
    /// The last knowledge response, pretty-printed and bounded.
    pub(crate) knowledge_output: Option<String>,
    /// The last knowledge failure.
    pub(crate) knowledge_error: Option<String>,
    /// The last successful knowledge publication.
    pub(crate) knowledge_notice: Option<String>,
    /// Knowledge items this Node currently serves.
    pub(crate) knowledge_items: Vec<FederationKnowledgeRow>,
    /// Files in the local artifact store.
    pub(crate) artifacts: Vec<FederationArtifactRow>,
    /// The capabilities chosen for a batch grant to the selected peer.
    pub(crate) grant_choices: std::collections::BTreeSet<String>,
    /// The compact pane's selected section: peers, work, resources, knowledge.
    pub(crate) tab: usize,
    /// Whether the raw/advanced controls are disclosed.
    pub(crate) advanced: bool,
    /// The unified operation log, newest last: every action the pane runs
    /// reports pending, running, succeeded, failed, revoked or cancelled here.
    pub(crate) activity: Vec<FederationActivity>,
    /// The next action id.
    pub(crate) activity_seq: u64,
    /// The operation a pending revoke confirmation owns, so the question can be
    /// settled as cancelled when the reader backs out.
    pub(crate) confirm_op: Option<u64>,
    /// The artifact the resources tab is previewing, read once on Open.
    pub(crate) preview: Option<FederationPreview>,
    /// Extension tools the host listed at the last refresh.
    pub(crate) extension_tools: Vec<crate::extension::host::NodeToolInfo>,
    /// The trust-state filter the peers card applies.
    pub(crate) peer_filter: PeerFilter,
}
/// The workspace browser's place in the tree, and the files open beside it.
///
/// The pane holds three things at once: the expandable directory tree the
/// reader walks, the directories it has listed so far, and the editor tabs
/// they opened out of it. Keeping all three is what makes the tabs worth
/// having - a reader can step back to the tree and open a second file without
/// losing the first, which is what the reference's dock does and what a file
/// browser that swaps its own body for an editor cannot do.
pub(crate) struct FilesPane {
    /// The workspace root's own listing, from which every branch descends.
    pub(crate) roots: Vec<files::Row>,
    /// The directories the reader has expanded, relative to the root, in the
    /// order they were opened so the chevrons never reshuffle under a hand.
    pub(crate) expanded: Vec<PathBuf>,
    /// Each expanded directory's listing, cached so a keystroke never walks
    /// the filesystem and a failed listing still shows what was there.
    pub(crate) children: HashMap<PathBuf, Vec<files::Row>>,
    /// The failure the last listing or mutation hit, which the pane reports.
    pub(crate) error: Option<String>,
    /// Whether hidden entries are listed.
    pub(crate) hidden: bool,
    /// The open editor tabs, in the order they were opened.
    pub(crate) tabs: Vec<FileTab>,
    /// Which tab the panel is showing, or none for the tree itself: the Files
    /// chip is the tree's own tab, so the tree and any open document are two
    /// surfaces the strip switches between, never two columns sharing a row.
    pub(crate) active: Option<usize>,
    /// The name typed into the New file field.
    pub(crate) new_name: Entity<InputState>,
    /// The entry being renamed, relative to the root, with the field that is
    /// renaming it. A rel rather than a name, because two branches can hold
    /// the same name and only one of them is being renamed.
    pub(crate) renaming: Option<PathBuf>,
    /// The rename field's engine, built once so a switching row keeps focus.
    pub(crate) rename_name: Entity<InputState>,
    /// The entry whose delete is armed, relative to the root: the first press
    /// arms it, the second deletes, which is the reference's own two-press
    /// guard. A rel rather than a name for the same reason `renaming` is.
    pub(crate) deleting: Option<PathBuf>,
    /// Whether the new-file field is showing. The tree keeps no permanent
    /// create row: a double-click on its blank space is what asks for one, and
    /// it goes away once the file is made or the tree is left.
    pub(crate) new_file_open: bool,
    /// Where and when the blank space was last pressed, so two presses close
    /// together in space and time read as a double-click.
    ///
    /// gpui's [`gpui::ClickEvent::click_count`] is the natural signal, but the
    /// test harness's synthetic clicks always report a count of one, and the
    /// gesture is worth driving from a test. So the blank handler tracks the
    /// prior press itself: a second press within [`SELF_DOUBLE_MS`] milliseconds
    /// and [`SELF_DOUBLE_PX`] pixels of the last one opens the field.
    pub(crate) last_blank_press: Option<(gpui::Point<gpui::Pixels>, std::time::Instant)>,
    /// The notes the reader has pinned to this workspace's files, loaded from
    /// the workspace's own `.bsh` directory and written back on every change.
    pub(crate) notes: crate::notes::Notes,
    /// Whether the notes drawer is showing over the document.
    pub(crate) notes_open: bool,
    /// Whether the inline comment card is up over the document.
    pub(crate) comment_open: bool,
    /// The comment the reader is reading, by index into the active file's
    /// notes. The thread panel under the editor shows this one and resumes its
    /// line in the code; none leaves the document unobstructed, which is the
    /// state browsing wants.
    pub(crate) focus_note: Option<usize>,
    /// The excerpt the thread panel draws: the focused note's current code,
    /// read once when the note is focused or its buffer changes. A frame must
    /// not clone a whole file to draw one panel.
    pub(crate) thread_quote: Option<(PathBuf, usize, String)>,
    /// The note being typed into the drawer's field.
    pub(crate) note_text: Entity<TextareaState>,
}

/// One file open in the editor area, as the tab strip carries it.
pub(crate) struct FileTab {
    /// Its path relative to the workspace root, which is what the pane joins
    /// onto the root for every later read and write.
    pub(crate) rel: PathBuf,
    /// The editing engine, or none for an image or a file the editor refused.
    pub(crate) editor: Option<Entity<EditorState>>,
    /// How it opened, so the save path and the fallback message agree.
    pub(crate) kind: Kind,
    /// Whether the buffer holds changes the disk does not.
    pub(crate) dirty: bool,
    /// The editor's comment marks: a soft fill under every line a note is
    /// pinned to. The collection tracks the buffer's own edits, so the marks
    /// ride the code through typing and scrolling instead of being recomputed
    /// per frame; it is created on the first note a file carries.
    pub(crate) decorations: Option<RangeDecorationCollection>,
    /// Which note each decoration range belongs to, in the collection's own
    /// order. The collection is the tracked truth about WHERE a comment sits
    /// after edits, so the rail reads its ranges instead of re-measuring the
    /// buffer every frame; this list is what maps a range back to its note.
    pub(crate) note_marks: Vec<usize>,
    /// Whether a Markdown tab shows its RENDERED document instead of its
    /// source. The source stays the editable truth: comments, save and undo
    /// all act on it, and the preview only decides what is painted.
    pub(crate) preview: bool,
    /// The parsed document the preview draws, built when the tab opened on
    /// Markdown or when the reader switched to it. None on every other file.
    pub(crate) document: Option<Entity<TextViewState>>,
    /// The rasterized picture an SVG tab previews, with the document's own
    /// dimensions for its caption. Built at open and rebuilt from the buffer
    /// whenever the preview comes back up, so an edited SVG previews its edit.
    pub(crate) svg: Option<SvgPreview>,
}

/// A rasterized SVG, ready for the image element.
pub(crate) struct SvgPreview {
    /// The document's own width, in user units (what the caption reports).
    pub(crate) width: u32,
    /// The document's own height, in user units.
    pub(crate) height: u32,
    /// The file's size on disk, for the same caption the rasters use.
    pub(crate) bytes: u64,
    /// The colour raster the pane paints.
    pub(crate) frame: std::sync::Arc<gpui_kit::RenderImage>,
}

impl FileTab {
    /// The name the tab is labelled with: the file's own name, not its path,
    /// because a tab bar that spelled out every path would be unreadable at
    /// the panel's width. The full path stays in the editor header above it.
    pub(crate) fn label(&self) -> String {
        self.rel
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| self.rel.display().to_string())
    }
}

impl FilesPane {
    /// The pane as it opens: at the workspace root, with nothing listed and no
    /// tabs, which is the state the reference's own dock starts in.
    pub(crate) fn new(
        new_name: Entity<InputState>,
        rename_name: Entity<InputState>,
        note_text: Entity<TextareaState>,
    ) -> Self {
        Self {
            roots: Vec::new(),
            expanded: Vec::new(),
            children: HashMap::new(),
            error: None,
            hidden: false,
            tabs: Vec::new(),
            active: None,
            new_name,
            renaming: None,
            rename_name,
            deleting: None,
            new_file_open: false,
            last_blank_press: None,
            notes: crate::notes::Notes::default(),
            notes_open: false,
            comment_open: false,
            focus_note: None,
            thread_quote: None,
            note_text,
        }
    }

    /// The tab the panel is showing, if any.
    pub(crate) fn active_tab(&self) -> Option<&FileTab> {
        self.active.and_then(|index| self.tabs.get(index))
    }

    /// The index of the tab already showing `rel`, if any, so opening a file a
    /// second time focuses it instead of stacking a duplicate.
    pub(crate) fn tab_index(&self, rel: &Path) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.rel == rel)
    }
}

/// Turn what the reader typed into a URL worth asking the policy about.
///
/// A bare host gets https, which is what a browser does when the scheme is
/// omitted: "example.com" is a request the reader meant, while "file:/x" and
/// "javascript:..." keep their scheme and are refused by the transport's own
/// rule with a sentence naming the scheme.
fn normalize_address(typed: &str) -> String {
    let text = typed.trim();
    if text.is_empty() {
        return String::new();
    }
    if text.contains("://") || text.to_ascii_lowercase().starts_with("http:") {
        return text.to_string();
    }
    format!("https://{}", text.trim_start_matches('/'))
}

/// The gap the strip puts between two chips.
///
/// Every width the overflow planner uses is this same figure, because the bar
/// draws its children with this gap: a plan that forgot it would fit tabs on
/// paper and then paint one gap past the edge it was respecting.
pub(crate) const STRIP_GAP: f32 = 4.0;

/// The width of one pane's launcher chip while that pane is not the open one.
///
/// A launcher is only its glyph; the label belongs to the open tab and to the
/// catalog, so the six panes that are not up stay narrow enough to leave the
/// documents the room they need.
pub(crate) const STRIP_PANE_CHIP_W: f32 = 26.0;

/// The width of the catalog trigger that replaces the chips that do not fit.
pub(crate) const STRIP_OVERFLOW_W: f32 = 26.0;

/// The width of one header control: new tab, maximise, collapse.
const STRIP_CONTROL_W: f32 = 26.0;

/// The narrowest an open tab may get before it moves into the catalog.
///
/// Finder stops shrinking a tab while the file glyph, the label's first
/// characters and the close are all still hittable; below this a tab is a
/// button with no name, so the catalog is the better home for it.
pub(crate) const STRIP_TAB_MIN_W: f32 = 72.0;

/// The narrowest an OPEN tab without a visible close may get.
///
/// Only the tab on screen draws a close, exactly as the reference does, so an
/// inactive tab does not reserve the 20px the close would take. That room goes
/// to the label instead, which is why a strip of nine tabs can still show four
/// readable names instead of four single letters.
pub(crate) const STRIP_TAB_MIN_PLAIN_W: f32 = 64.0;

/// The widest floor a two-word label may reserve. A single very long word is
/// clipped to the floor rather than starving every other tab.
const STRIP_TAB_FLOOR_MAX_W: f32 = 120.0;

/// The widest an open tab grows before its label is ellipsised.
pub(crate) const STRIP_TAB_MAX_W: f32 = 168.0;

/// What the three header controls and the gaps around them reserve at the
/// right edge, including the gap before the strip's own trailing spacer.
fn strip_chrome_w(controls: usize) -> f32 {
    STRIP_CONTROL_W * controls as f32 + STRIP_GAP * (controls + 1) as f32
}

/// The width a chip label needs at the chip size, before it is clipped.
///
/// A CJK glyph is a full em and an ASCII one a little over half, so a
/// per-character estimate is honest in both locales. It only picks a tab's
/// width: the chip clips its own label, so a long name can never overrun the
/// strip even when the estimate is a pixel or two short.
fn chip_label_w(label: &str) -> f32 {
    label
        .chars()
        .map(|ch| {
            if ch.is_ascii() {
                theme::CHIP_SIZE * 0.53
            } else {
                theme::CHIP_SIZE
            }
        })
        .sum()
}

/// The width an open pane's tab asks for: padding, glyph, gap, label, close.
#[cfg(test)]
pub(crate) fn pane_tab_w(label: &str) -> f32 {
    pane_tab_w_with(label, true)
}

/// The width a pane tab asks for, with or without the close it draws only
/// while selected: the close costs a 2px margin, 6px of padding and a 12px
/// glyph, so an inactive tab gets that space back for its label.
pub(crate) fn pane_tab_w_with(label: &str, close: bool) -> f32 {
    let base = if close { 62.0 } else { 36.0 };
    let min = if close {
        STRIP_TAB_MIN_W
    } else {
        STRIP_TAB_MIN_PLAIN_W
    };
    (base + chip_label_w(label)).clamp(min, STRIP_TAB_MAX_W)
}

/// The width a document tab asks for: padding, badge, gap, label, close.
#[cfg(test)]
pub(crate) fn file_tab_w(label: &str) -> f32 {
    file_tab_w_with(label, true)
}

/// The width a document tab asks for, with or without its close.
pub(crate) fn file_tab_w_with(label: &str, close: bool) -> f32 {
    // 8px padding a side, an 18px type badge and a 6px gap leave 40px before
    // the label; the close adds 17px and its 6px gap.
    let base = if close { 64.0 } else { 41.0 };
    let min = if close {
        STRIP_TAB_MIN_W
    } else {
        STRIP_TAB_MIN_PLAIN_W
    };
    (base + chip_label_w(label)).clamp(min, STRIP_TAB_MAX_W)
}

/// The label room a tab must keep before it may be clipped: the first two
/// words, or a one-word name up to twelve characters, plus the ellipsis.
fn tab_label_floor(label: &str) -> f32 {
    let mut prefix = String::new();
    for word in label.split_whitespace().take(2) {
        if !prefix.is_empty() {
            prefix.push(' ');
        }
        prefix.push_str(word);
    }
    if prefix.is_empty() {
        prefix = label.to_string();
    }
    let clipped = prefix.chars().count() > 12;
    let mut chars: Vec<char> = prefix.chars().take(12).collect();
    if clipped {
        chars.push('…');
    }
    let text: String = chars.into_iter().collect();
    chip_label_w(&text) + 8.0
}

/// The narrowest a pane tab may draw while keeping its two-word label.
pub(crate) fn pane_tab_floor_with(label: &str, close: bool) -> f32 {
    let base = if close { 62.0 } else { 36.0 };
    (base + tab_label_floor(label)).min(STRIP_TAB_FLOOR_MAX_W)
}

/// The narrowest a document tab may draw while keeping its two-word label.
pub(crate) fn file_tab_floor_with(label: &str, close: bool) -> f32 {
    let base = if close { 64.0 } else { 41.0 };
    (base + tab_label_floor(label)).min(STRIP_TAB_FLOOR_MAX_W)
}

/// The category filter in Finder's Show All Tabs catalogue.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CatalogFilter {
    /// Every surface and document.
    #[default]
    All,
    /// The documents opened out of Files.
    Documents,
    /// The open pane, or Start when the pane is closed, and every pane the
    /// catalogue can switch to.
    Panes,
}

impl CatalogFilter {
    /// The filter's label key in the string table.
    pub(crate) fn key(self) -> &'static str {
        match self {
            CatalogFilter::All => "right.tabs.all",
            CatalogFilter::Documents => "right.tabs.documents",
            CatalogFilter::Panes => "right.tabs.panes",
        }
    }

    /// The filter's stable id stem, for its control and the tests.
    pub(crate) fn slug(self) -> &'static str {
        match self {
            CatalogFilter::All => "all",
            CatalogFilter::Documents => "documents",
            CatalogFilter::Panes => "panes",
        }
    }

    /// Whether this filter admits the entry.
    fn admits(self, entry: CatalogEntry) -> bool {
        match self {
            CatalogFilter::All => true,
            CatalogFilter::Documents => matches!(entry, CatalogEntry::File(_)),
            CatalogFilter::Panes => !matches!(entry, CatalogEntry::File(_)),
        }
    }
}

/// Which right-pane items the top search box offers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RightSearchScope {
    /// Panes and documents.
    #[default]
    All,
    /// The pane items: Files, Diff, Terminal, Browser, Extensions, Federation,
    /// Tasks.
    Panes,
    /// The documents opened out of Files.
    Documents,
}

impl RightSearchScope {
    /// The scope's label key in the string table.
    pub(crate) fn key(self) -> &'static str {
        match self {
            RightSearchScope::All => "right.tabs.all",
            RightSearchScope::Panes => "right.tabs.panes",
            RightSearchScope::Documents => "right.tabs.documents",
        }
    }

    /// The scope's stable id stem, for its menu row and the tests.
    pub(crate) fn slug(self) -> &'static str {
        match self {
            RightSearchScope::All => "all",
            RightSearchScope::Panes => "panes",
            RightSearchScope::Documents => "documents",
        }
    }
}

/// One right-pane item the top search box can offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RightSearchEntry {
    /// A pane, opened by choosing the result.
    Pane(RightTab),
    /// A document opened out of Files.
    File(usize),
}

/// One entry the Show All Tabs catalogue can draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CatalogEntry {
    /// The Start screen, when no pane is open.
    Start,
    /// A pane, whether it is the open one or a pane the catalogue can switch to.
    Pane(RightTab),
    /// A document opened out of Files.
    File(usize),
}

/// One tab the strip can draw: the open pane, or a document opened in Files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StripSlot {
    /// The Start screen, the panel's own new-tab page.
    Start,
    Pane(RightTab),
    File(usize),
}

/// The tabs that fit this frame, in strip order, and whether some did not.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StripPlan {
    /// What to draw, each with the width it was given.
    pub(crate) visible: Vec<(StripSlot, f32)>,
    /// Whether the catalog trigger must stand in for the tabs left out.
    pub(crate) overflow: bool,
}

/// Decide which tabs the strip can show at a given width.
///
/// The priority is Finder's: the tab that is open first, then the documents the
/// reader opened, and the launcher chips for panes that are not up last. That
/// order is what keeps a wide pane from starving the documents: a document
/// whose name no longer fits moves into the catalog before any tab is drawn
/// narrower than legibility allows.
///
/// The open document is never hidden, even when the window is too narrow for
/// its floor: a strip with no tab on screen has nothing to name the body with.
/// That is the one branch where a width may exceed the available room, and the
/// panel's own 320px floor keeps it out of reach in the real window.
#[allow(dead_code)]
pub(crate) fn plan_tab_strip(
    available: f32,
    active_pane: Option<RightTab>,
    active_pane_w: f32,
    file_widths: &[f32],
    active_file: Option<usize>,
    inactive_panes: &[RightTab],
) -> StripPlan {
    let mut visible = Vec::with_capacity(1 + file_widths.len() + inactive_panes.len());
    let lead = match active_pane {
        Some(pane) => StripSlot::Pane(pane),
        None => StripSlot::Start,
    };
    visible.push((lead, active_pane_w));
    let mut used = active_pane_w;
    let mut overflow = false;

    if !file_widths.is_empty() {
        let count = file_widths.len();
        let natural: f32 = file_widths.iter().sum();
        let gaps = count as f32 * STRIP_GAP;
        if used + natural + gaps <= available {
            for (index, width) in file_widths.iter().enumerate() {
                used += STRIP_GAP + width;
                visible.push((StripSlot::File(index), *width));
            }
        } else if used + STRIP_TAB_MIN_W * count as f32 + gaps <= available {
            for index in 0..count {
                used += STRIP_GAP + STRIP_TAB_MIN_W;
                visible.push((StripSlot::File(index), STRIP_TAB_MIN_W));
            }
        } else {
            overflow = true;
            let area = (available - used - STRIP_OVERFLOW_W - STRIP_GAP).max(0.0);
            let active = active_file.unwrap_or(0).min(count - 1);
            let mut start = active;
            let mut end = active;
            let mut cost = file_widths[active] + STRIP_GAP;
            while start > 0 && cost + STRIP_GAP + file_widths[start - 1] <= area {
                start -= 1;
                cost += STRIP_GAP + file_widths[start];
            }
            while end + 1 < count && cost + STRIP_GAP + file_widths[end + 1] <= area {
                end += 1;
                cost += STRIP_GAP + file_widths[end];
            }
            let shown = end - start + 1;
            let per = ((area - shown as f32 * STRIP_GAP) / shown as f32)
                .clamp(STRIP_TAB_MIN_W, STRIP_TAB_MAX_W);
            for (index, natural) in file_widths.iter().enumerate().take(end + 1).skip(start) {
                let width = natural.min(per);
                used += STRIP_GAP + width;
                visible.push((StripSlot::File(index), width));
            }
        }
    }

    if !inactive_panes.is_empty() {
        let pane_cost = STRIP_PANE_CHIP_W + STRIP_GAP;
        let room = available
            - used
            - if overflow {
                STRIP_OVERFLOW_W + STRIP_GAP
            } else {
                0.0
            };
        let mut fit = (room / pane_cost).floor().max(0.0) as usize;
        if fit < inactive_panes.len() {
            overflow = true;
            let room = available - used - STRIP_OVERFLOW_W - STRIP_GAP;
            fit = (room / pane_cost).floor().max(0.0) as usize;
        }
        for pane in inactive_panes.iter().take(fit) {
            visible.push((StripSlot::Pane(*pane), STRIP_PANE_CHIP_W));
        }
    }

    StripPlan { visible, overflow }
}

/// Choose which open tabs the strip can show, in order, around the active one.
///
/// Unlike the old launcher planner, every item here is an OPEN tab: Start (when
/// it is the active new-tab page), the pane tabs the reader opened, and the
/// document tabs. All of them carry labels and closes, so they are packed at
/// the same width floor; when even the floor does not fit, the window closes
/// around the tab on screen and the rest move into the catalog.
fn plan_open_strip(
    available: f32,
    start: Option<(f32, f32)>,
    panes: &[(RightTab, f32, f32)],
    files: &[(f32, f32)],
    active: Option<StripSlot>,
) -> StripPlan {
    let mut items: Vec<(StripSlot, f32, f32)> = Vec::new();
    if let Some((width, floor)) = start {
        items.push((StripSlot::Start, width, floor));
    }
    for (tab, width, floor) in panes {
        items.push((StripSlot::Pane(*tab), *width, *floor));
    }
    for (index, (width, floor)) in files.iter().enumerate() {
        items.push((StripSlot::File(index), *width, *floor));
    }
    if items.is_empty() {
        return StripPlan {
            visible: Vec::new(),
            overflow: false,
        };
    }
    let total = |widths: &[f32]| {
        widths.iter().sum::<f32>() + STRIP_GAP * widths.len().saturating_sub(1) as f32
    };
    let natural: Vec<f32> = items.iter().map(|(_, width, _)| *width).collect();
    if total(&natural) <= available {
        return StripPlan {
            visible: items
                .into_iter()
                .map(|(slot, width, _)| (slot, width))
                .collect(),
            overflow: false,
        };
    }
    // Every visible tab keeps its two-word label before the planner clips it;
    // a floor never exceeds the tab's natural width.
    let floors: Vec<f32> = items
        .iter()
        .map(|(_, width, floor)| floor.min(*width))
        .collect();
    let active_index = active
        .and_then(|slot| {
            items
                .iter()
                .position(|(candidate, _, _)| *candidate == slot)
        })
        .unwrap_or(0);
    if total(&floors) <= available {
        let widths = distribute_strip_width(available, &natural, &floors, active_index);
        return StripPlan {
            visible: items
                .into_iter()
                .zip(widths)
                .map(|((slot, _, _), width)| (slot, width))
                .collect(),
            overflow: false,
        };
    }
    let area = (available - STRIP_OVERFLOW_W - STRIP_GAP).max(0.0);
    let mut start_index = active_index;
    let mut end_index = active_index;
    let mut cost = floors[active_index];
    while start_index > 0 && cost + STRIP_GAP + floors[start_index - 1] <= area {
        start_index -= 1;
        cost += STRIP_GAP + floors[start_index];
    }
    while end_index + 1 < items.len() && cost + STRIP_GAP + floors[end_index + 1] <= area {
        end_index += 1;
        cost += STRIP_GAP + floors[end_index];
    }
    let window_natural = &natural[start_index..=end_index];
    let window_floors = &floors[start_index..=end_index];
    let widths = distribute_strip_width(
        area,
        window_natural,
        window_floors,
        active_index.saturating_sub(start_index),
    );
    let visible = (start_index..=end_index)
        .zip(widths)
        .map(|(index, width)| (items[index].0, width))
        .collect();
    StripPlan {
        visible,
        overflow: true,
    }
}

/// Fill the room the strip actually has: every tab starts at its floor, the
/// selected tab takes its label next, and the rest share what is left before
/// any label is clipped.
fn distribute_strip_width(
    available: f32,
    natural: &[f32],
    mins: &[f32],
    active: usize,
) -> Vec<f32> {
    let count = natural.len();
    if count == 0 {
        return Vec::new();
    }
    let gaps = STRIP_GAP * count.saturating_sub(1) as f32;
    let mut widths = mins.to_vec();
    let mut remaining = (available - gaps - widths.iter().sum::<f32>()).max(0.0);
    if remaining < 0.5 {
        return widths;
    }
    if let Some(width) = widths.get_mut(active) {
        let add = (natural[active] - *width).max(0.0).min(remaining);
        *width += add;
        remaining -= add;
    }
    while remaining >= 0.5 {
        let open: Vec<usize> = (0..count)
            .filter(|index| widths[*index] + 0.5 < natural[*index])
            .collect();
        if open.is_empty() {
            break;
        }
        let share = remaining / open.len() as f32;
        let mut used = 0.0;
        for index in open {
            let add = (natural[index] - widths[index]).min(share);
            if add > 0.0 {
                widths[index] += add;
                used += add;
            }
        }
        if used < 0.5 {
            break;
        }
        remaining -= used;
    }
    widths
}

/// One row of the tab catalog: the tab's glyph, its label, and the check that
/// says which one the body is showing.
///
/// The kit's own item draws a plain label and cannot carry the assets crate's
/// glyphs, so the row is an element the same way the workspace menu's rows are.
fn catalog_row(selector: String, glyph: IconName, label: String, active: bool) -> PopupMenuItem {
    PopupMenuItem::element(move |_window, _cx| {
        let selector = selector.clone();
        let color = if active {
            theme::text()
        } else {
            theme::muted()
        };
        div()
            .debug_selector(move || selector.clone())
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(icon(glyph, 13.0, color).flex_none())
            .child(
                txt(label.clone(), 13.0, color)
                    .flex_1()
                    .min_w_0()
                    .text_ellipsis(),
            )
            .when(active, |row| {
                row.child(icon(IconName::Check, 12.0, theme::accent()))
            })
    })
}

impl Shell {
    /// Whether the panel is the column at the window's top-left corner.
    ///
    /// The panel is the LAST column of the row, so it only reaches x=0 when the
    /// sidebar is folded AND the panel is wide enough to squeeze the main
    /// column to nothing - a maximised panel asks for exactly the row's width,
    /// which with no sidebar is the whole window. That is the state the band is
    /// for: the panel's tab strip shares the traffic-light strip and starts
    /// after the lights. A plain (non-maximised) panel is capped at half its
    /// space, so it always has the main column beside it and never owns the
    /// corner. The zero-width main column is not rendered in this state; see
    /// `Shell::render`.
    pub(crate) fn right_panel_owns_the_corner(&self) -> bool {
        self.right.open
            && self.sidebar_folded()
            && self.viewport_w > 0.0
            && self.right_panel_w() >= self.viewport_w
    }

    /// The titlebar band the panel owns when it is the window's corner column.
    ///
    /// The band IS the bar: the tab strip is drawn in the same 52px the
    /// traffic lights sit in, so the panel reads as a titlebar rather than an
    /// empty strip with the tabs pushed below it. The bar starts after the
    /// lights; the stretch left over - exactly the inset's width - is the
    /// drag region, so the corner still moves the window while the lights stay
    /// uncovered and clickable (macOS draws them over the content, and this
    /// strip only asks for the move). Only the column that owns the corner
    /// draws it - see [`Shell::right_panel_owns_the_corner`] - which is why a
    /// folded panel leaves no dead hole behind: the corner goes back to the
    /// sidebar or the main column, which own it in their own right.
    fn right_panel_titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let zone = theme::traffic_light_inset();
        div()
            .debug_selector(|| String::from("titlebar-band"))
            .w_full()
            .h(px(zone.bottom))
            .flex_none()
            .flex()
            .flex_row()
            .child(
                div()
                    .debug_selector(|| String::from("titlebar-drag"))
                    .id("titlebar-drag")
                    .w(px(zone.right))
                    .h_full()
                    .flex_none()
                    .on_mouse_down(MouseButton::Left, |_event, window, _cx| {
                        // The same call AppKit's titlebar makes for a
                        // system-dragged window: the band reads as the title
                        // bar, not as a control.
                        window.start_window_move();
                    }),
            )
            .child(self.right_panel_bar(cx, true))
    }

    /// The right panel column and its resize strip, absent while it is folded.
    pub(crate) fn render_right_panel(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !self.right.open {
            return None;
        }
        let width = self.right_panel_w();
        // The panel is the last column, so it normally sits beside the main
        // column and away from the window's corner. The one state where it owns
        // that corner is the maximised panel with the sidebar folded, and there
        // its header takes the traffic-light band the sidebar gave up, through
        // the one shared inset, like the header does with its folded left pad.
        let owns_corner = self.right_panel_owns_the_corner();
        let column = div()
            .debug_selector(|| String::from("right-panel"))
            // The panel is a fixed column BESIDE a flex-1 main column, so a
            // width that does not fit does not shrink the chat - the row
            // overflows the window and the panel's right edge lands off screen,
            // taking its controls with it. The resolved width cannot know its
            // sibling, so the floor of a real panel is enforced here, where the
            // flex row is the thing doing the fitting.
            .min_w(px(theme::RIGHT_PANEL_MIN))
            .w(px(width))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(theme::page()))
            // One header, in one of its two placements: the band (bar and drag
            // region sharing the traffic-light strip) when the panel owns the
            // corner, or the plain 40px bar below it when it does not. They are
            // alternatives, never both, so the strip is reserved exactly once.
            .when(owns_corner, |column| {
                column.child(self.right_panel_titlebar(cx))
            })
            .when(!owns_corner, |column| {
                column.child(self.right_panel_bar(cx, false))
            })
            // The reference hangs the same `.5px` rule on this seam that it
            // hangs on the sidebar's, so the panel reads as a column of the
            // same family.
            .child(self.right_panel_body(cx));
        Some(
            div()
                .debug_selector(|| String::from("right-panel-frame"))
                .relative()
                .w(px(width))
                .h_full()
                .flex_shrink_0()
                // Moving anywhere else in the column puts the bar away; the
                // handle's own move stops propagation, so the two never fight.
                .on_mouse_move(cx.listener(|this, _event: &MouseMoveEvent, _window, cx| {
                    this.hover_right_seam(false, cx);
                }))
                .child(hairline_left(column).border_color(rgb(theme::border_soft())))
                .child(self.render_right_panel_handle(cx)),
        )
    }

    /// The reference's DragHandle for the panel: an 8px col-resize strip
    /// straddling the seam, drawn only while the column is up, carrying the
    /// same reveal bar the content column's handles do: invisible until the
    /// pointer is on the seam (or a drag holds it), then the measured 2x72.
    fn render_right_panel_handle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("right-panel-handle")
            .debug_selector(|| String::from("right-panel-handle"))
            .occlude()
            .absolute()
            .top(px(0.0))
            .bottom(px(0.0))
            .left(px(-crate::theme::SIDEBAR_HANDLE_W / 2.0))
            .w(px(crate::theme::SIDEBAR_HANDLE_W))
            .flex()
            .items_center()
            .justify_center()
            .cursor(CursorStyle::ResizeLeftRight)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    this.begin_right_panel_drag(event.position.x.as_f32(), cx);
                }),
            )
            // The bar follows the pointer; stopping the move here is what keeps
            // the column's own move from clearing it in the same dispatch.
            .on_mouse_move(cx.listener(|this, _event: &MouseMoveEvent, _window, cx| {
                this.hover_right_seam(true, cx);
                cx.stop_propagation();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                    this.commit_right_panel_width(event.position.x.as_f32(), cx);
                }),
            )
            .child(handle_bar(
                "right-panel-bar",
                if self.right.drag.is_some() || self.right_hover {
                    1.0
                } else {
                    0.0
                },
            ))
    }

    /// The panel's bar: the Start screen's tab, or the open pane's strip, and
    /// the controls the reference draws on every panel header.
    ///
    /// `band` selects the placement, but both share ONE 28px content row, so a
    /// 28px tab chip, a 26px file chip and a 26px control share one centre
    /// line. The plain 40px header centres that row at y=6 (the open-sidebar
    /// state); the traffic-light band starts it at
    /// [`theme::RIGHT_BAR_BAND_PAD`] (10) so the row's centre is the window
    /// buttons' own centre line (24), not the 52px strip's centre (26) - a
    /// screenshot showed the row sitting high against the lights.
    fn right_panel_bar(&self, cx: &mut Context<Self>, band: bool) -> impl IntoElement {
        let bar = div()
            // The header, not the seam's reveal bar: that one already carries
            // "right-panel-bar", and two elements sharing a selector made the
            // strip's bounds unreadable (debug_bounds returned the 2x72 bar).
            .debug_selector(|| String::from("right-panel-header"))
            .flex()
            .flex_row()
            .items_start()
            .px(px(theme::RIGHT_PAD))
            .when(band, |bar| {
                bar.h(px(theme::traffic_light_inset().bottom))
                    .pt(px(theme::RIGHT_BAR_BAND_PAD))
                    .flex_1()
                    .min_w_0()
            })
            .when(!band, |bar| {
                bar.h(px(theme::RIGHT_BAR_H))
                    .pt(px(theme::RIGHT_BAR_TOP_PAD))
                    .flex_none()
            });
        // The one row every placement uses: its 28px height is what gives the
        // 28px and 26px chips the same vertical centre inside it.
        let row = div()
            .h(px(theme::RIGHT_TAB_H))
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.0));
        // The strip is the set of OPEN tabs: Start when it is the active
        // new-tab page, every pane the reader opened, and every document. The
        // active tab stays visible; the rest are one click away, and anything
        // that does not fit is in the catalog at the trailing edge.
        let active_pane = self.right.tab;
        let mut open_panes = self.right.open_panes.clone();
        if let Some(tab) = active_pane {
            if !open_panes.contains(&tab) {
                open_panes.push(tab);
            }
        }
        let panes: Vec<(RightTab, f32, f32)> = open_panes
            .iter()
            .map(|tab| {
                // Only the selected tab draws a close, so only it reserves the
                // close's width; the inactive labels get that room.
                let close = self.right.tab == Some(*tab)
                    && !(*tab == RightTab::Files && self.files.active.is_some());
                let label = self.state.lang.tr(tab.key());
                (
                    *tab,
                    pane_tab_w_with(&label, close),
                    pane_tab_floor_with(&label, close),
                )
            })
            .collect();
        let file_widths: Vec<(f32, f32)> = self
            .files
            .tabs
            .iter()
            .enumerate()
            .map(|(index, file)| {
                let close =
                    self.right.tab == Some(RightTab::Files) && self.files.active == Some(index);
                let mut width = file_tab_w_with(&file.label(), close);
                let mut floor = file_tab_floor_with(&file.label(), close);
                // The dirty dot and the notes glyph are real children of the
                // chip; leaving them out of the estimate is what clipped the
                // selected file's name behind its own close.
                if file.dirty {
                    width += 12.0;
                    floor += 12.0;
                }
                if self.files.notes.has_notes(&file.rel) {
                    width += 17.0;
                    floor += 17.0;
                }
                (width.min(STRIP_TAB_MAX_W), floor.min(width))
            })
            .collect();
        let start = active_pane.is_none().then(|| {
            let label = self.state.lang.tr("right.start");
            (
                pane_tab_w_with(&label, false),
                pane_tab_floor_with(&label, false),
            )
        });
        let active = match active_pane {
            Some(RightTab::Files) if self.files.active.is_some() => {
                self.files.active.map(StripSlot::File)
            }
            Some(tab) => Some(StripSlot::Pane(tab)),
            None => Some(StripSlot::Start),
        };
        // The comment control is a document's own, and so is the preview
        // switch: both appear with their document and share the strip's
        // trailing budget with the other three.
        let comments = self.comments_control_shown();
        let preview = self.preview_control_shown();
        let plan = plan_open_strip(
            self.strip_budget(band, 3 + usize::from(comments) + usize::from(preview)),
            start,
            &panes,
            &file_widths,
            active,
        );
        let mut strip = div()
            .h(px(theme::RIGHT_TAB_H))
            .flex_1()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(STRIP_GAP));
        for (slot, width) in plan.visible {
            let chip = match slot {
                StripSlot::Start => self.start_tab_chip(width, cx).into_any_element(),
                StripSlot::Pane(pane) => self.right_tab_chip(pane, width, cx).into_any_element(),
                StripSlot::File(index) => match self.files.tabs.get(index) {
                    Some(file_tab) => self
                        .files_tab_chip(index, file_tab, width, cx)
                        .into_any_element(),
                    None => continue,
                },
            };
            strip = strip.child(chip);
        }
        strip = strip.child(div().flex_1());
        if plan.overflow {
            strip = strip.child(self.tab_catalog(cx));
        }
        let content = row
            .child(strip)
            .child(self.right_new_tab(cx))
            .when(preview, |row| row.child(self.right_preview_control(cx)))
            .when(comments, |row| row.child(self.right_comments_control(cx)))
            .child(self.right_panel_control(
                "right-maximize",
                if self.right.maximized {
                    IconName::Minimize2
                } else {
                    IconName::Maximize2
                },
                cx,
            ))
            .child(self.right_panel_close(cx));
        bar.child(content)
    }

    /// Whether the strip carries the document's preview switch.
    fn preview_control_shown(&self) -> bool {
        self.right.tab == Some(RightTab::Files)
            && self
                .files
                .active_tab()
                .is_some_and(|tab| renders_preview(&tab.rel))
    }

    /// The Markdown tab's switch between the rendered document and its source.
    ///
    /// The icon names the state the press will produce (a book while the source
    /// is up, code while the preview is), so the control reads as an action
    /// rather than as a status light.
    fn right_preview_control(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        let preview = self.files.active_tab().is_some_and(|tab| tab.preview);
        let label = lang.tr(if preview {
            "right.files.source"
        } else {
            "right.files.preview"
        });
        div()
            .id("right-preview")
            .debug_selector(|| String::from("right-preview"))
            .w(px(26.0))
            .h(px(26.0))
            .flex_none()
            .rounded(px(theme::RADIUS_SM))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .when(preview, |control| control.bg(rgb(theme::selected_row())))
            .hover(|s| s.bg(rgb(theme::hover())))
            .tooltip_show_delay(std::time::Duration::from_millis(400))
            .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
            .child(icon(
                if preview {
                    IconName::Code
                } else {
                    IconName::BookOpen
                },
                14.0,
                theme::muted(),
            ))
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.toggle_preview(cx);
            }))
    }

    /// Whether the strip carries the document's comment control.
    fn comments_control_shown(&self) -> bool {
        self.right.tab == Some(RightTab::Files) && self.files.active_tab().is_some()
    }

    /// The document's comment control: how many remarks are open, and the door
    /// to the drawer that lists them.
    ///
    /// It lives in the strip because comments belong to the document the strip
    /// names, and it carries a dot rather than a count so a narrow panel never
    /// has to clip the control that says whether anything is pinned.
    fn right_comments_control(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        let open = self
            .files
            .active_tab()
            .map(|tab| self.files.notes.open_count(&tab.rel))
            .unwrap_or(0);
        let total = self
            .files
            .active_tab()
            .map(|tab| self.files.notes.on(&tab.rel).len())
            .unwrap_or(0);
        let label = format!("{} ({open}/{total})", lang.tr("right.files.notes"));
        let active = self.files.notes_open;
        div()
            .id("right-comments")
            .debug_selector(|| String::from("right-comments"))
            .w(px(26.0))
            .h(px(26.0))
            .flex_none()
            .relative()
            .rounded(px(theme::RADIUS_SM))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .when(active, |control| control.bg(rgb(theme::selected_row())))
            .hover(|s| s.bg(rgb(theme::hover())))
            .tooltip_show_delay(std::time::Duration::from_millis(400))
            .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
            .child(icon(
                IconName::MessageSquare,
                14.0,
                if open > 0 {
                    theme::text()
                } else {
                    theme::faint()
                },
            ))
            .when(open > 0, |control| {
                control.child(
                    div()
                        .absolute()
                        .top(px(5.0))
                        .right(px(5.0))
                        .w(px(4.0))
                        .h(px(4.0))
                        .rounded_full()
                        .bg(rgb(theme::accent())),
                )
            })
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.files.notes_open = !this.files.notes_open;
                cx.notify();
            }))
    }

    /// The pixels the strip itself may use, after the padding and the three
    /// header controls have taken theirs.
    ///
    /// The panel owns the traffic-light band when it is the window's corner
    /// column, and then its bar starts after the lights; the budget has to
    /// follow that, or the plan would fit one last tab under the buttons.
    fn strip_budget(&self, band: bool, controls: usize) -> f32 {
        let bar_w = if band {
            (self.right_panel_w() - theme::traffic_light_inset().right).max(0.0)
        } else {
            self.right_panel_w()
        };
        // The bar's 12px padding a side, the column's two .5px rules, and the
        // controls with the gaps around them and at the strip's trailing edge.
        (bar_w - theme::RIGHT_PAD * 2.0 - 1.0 - strip_chrome_w(controls)).max(0.0)
    }

    /// The Start screen's own tab: the panel's new-tab page.
    ///
    /// It is the one tab with no close - closing it is what the panel's collapse
    /// control does, and a Start tab that can be closed would leave the strip
    /// with no surface at all. Its menu still carries the document operations,
    /// so a reader who closed the pane can clear the documents from here.
    fn start_tab_chip(&self, width: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        let label = lang.tr("right.start");
        let documents = self.files.tabs.len();
        let entity = cx.entity();
        div()
            .id("right-start-tab")
            .debug_selector(|| String::from("right-start-tab"))
            .w(px(width))
            .h(px(theme::RIGHT_TAB_H))
            .flex_none()
            .min_w_0()
            .px(px(8.0))
            .rounded(px(theme::RADIUS_SM))
            .bg(rgb(theme::selected_row()))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .overflow_hidden()
            .child(icon(IconName::Compass, 14.0, theme::text()).flex_none())
            .child(
                txt(label, theme::CHIP_SIZE, theme::text())
                    .font_weight(FontWeight::MEDIUM)
                    .flex_1()
                    .min_w_0()
                    .text_ellipsis(),
            )
            .context_menu(move |menu, _, _| {
                let all = entity.clone();
                let show = entity.clone();
                menu.item(
                    PopupMenuItem::new(lang.tr("right.closeAll"))
                        .disabled(documents == 0)
                        .on_click(move |_, _, cx| {
                            all.update(cx, |this, cx| this.close_all_tabs(cx));
                        }),
                )
                .item(PopupMenuItem::separator())
                .item(
                    PopupMenuItem::new(lang.tr("right.showAllTabs"))
                        .disabled(documents == 0)
                        .on_click(move |_, window, cx| {
                            show.update(cx, |this, cx| this.open_tab_catalog(window, cx));
                        }),
                )
            })
    }

    /// The strip's new-tab control: a press lands on the Start screen, which is
    /// where a pane without a chip of its own is picked.
    fn right_new_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let label = self.state.lang.tr("right.newTab");
        div()
            .id("right-new-tab")
            .debug_selector(|| String::from("right-new-tab"))
            .w(px(26.0))
            .h(px(26.0))
            .flex_none()
            .rounded(px(theme::RADIUS_SM))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(theme::hover())))
            .tooltip_show_delay(std::time::Duration::from_millis(500))
            .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
            .child(icon(IconName::Plus, 14.0, theme::faint()))
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.show_start(cx);
            }))
    }

    /// The catalog behind the ellipsis: every open tab, then the actions that
    /// close them.
    ///
    /// This is the overflow menu Finder shows when a window cannot fit another
    /// tab, except that it catalogs the WHOLE strip rather than only the tabs
    /// that were left out: a reader who opens it can see where they are and
    /// move to any tab, not just recover the hidden ones. The strip only draws
    /// the trigger when something actually did not fit; hiding tabs behind a
    /// control that is always there would make the strip lie about its width.
    fn tab_catalog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        let entity = cx.entity();
        let docs: Vec<(usize, String, bool)> = self
            .files
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                (
                    index,
                    tab.label(),
                    self.right.tab == Some(RightTab::Files) && self.files.active == Some(index),
                )
            })
            .collect();
        let mut open_panes = self.right.open_panes.clone();
        if let Some(tab) = self.right.tab {
            if !open_panes.contains(&tab) {
                open_panes.push(tab);
            }
        }
        let panes: Vec<(RightTab, String, bool)> = open_panes
            .iter()
            .map(|pane| (*pane, lang.tr(pane.key()), self.right.tab == Some(*pane)))
            .collect();
        let total = docs.len() + panes.len();
        let can_close = self.right.tab.is_some() || self.files.active.is_some();
        let can_others = self.right.tab.is_some() && total > 1;
        let can_all = total > 0;
        let title = format!("{} ({})", lang.tr("right.tabs"), total);
        let tooltip = title.clone();
        // The kit's dropdown rides a Button; the wrapper is what carries the
        // debug selector, because a Button records no bounds of its own.
        let trigger = Button::new("right-tab-overflow")
            .ghost()
            .w(px(STRIP_OVERFLOW_W))
            .h(px(26.0))
            .p(px(0.0))
            .flex_none()
            .rounded(px(theme::RADIUS_SM))
            .flex()
            .items_center()
            .justify_center()
            .child(icon(IconName::Ellipsis, 14.0, theme::faint()))
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                let mut menu = menu.scrollable(true).max_h(px(420.0));
                menu = menu.item(PopupMenuItem::label(lang.tr("right.tabs")));
                if total == 0 {
                    menu = menu.item(PopupMenuItem::label(lang.tr("right.tabs.none")));
                }
                for (index, label, active) in docs.iter().cloned() {
                    let shell = entity.clone();
                    menu = menu.item(
                        catalog_row(
                            format!("tab-catalog-file-{index}"),
                            file_glyph_for_name(&label),
                            label,
                            active,
                        )
                        .on_click(move |_, _window, cx| {
                            shell.update(cx, |this, cx| this.focus_tab(index, cx));
                        }),
                    );
                }
                if !panes.is_empty() && !docs.is_empty() {
                    menu = menu
                        .item(PopupMenuItem::separator())
                        .item(PopupMenuItem::label(lang.tr("right.tabs.panes")));
                }
                for (pane, label, active) in panes.iter().cloned() {
                    let shell = entity.clone();
                    menu = menu.item(
                        catalog_row(
                            format!("tab-catalog-pane-{}", pane.slug()),
                            pane.glyph(),
                            label,
                            active,
                        )
                        .on_click(move |_, _window, cx| {
                            shell.update(cx, |this, cx| this.activate_pane(pane, cx));
                        }),
                    );
                }
                menu = menu.item(PopupMenuItem::separator());
                let close = entity.clone();
                menu = menu.item(
                    PopupMenuItem::new(lang.tr("right.close"))
                        .disabled(!can_close)
                        .on_click(move |_, _, cx| {
                            close.update(cx, |this, cx| this.close_active_tab(cx));
                        }),
                );
                let others = entity.clone();
                menu = menu.item(
                    PopupMenuItem::new(lang.tr("right.closeOthers"))
                        .disabled(!can_others)
                        .on_click(move |_, _, cx| {
                            others.update(cx, |this, cx| this.close_other_active_tabs(cx));
                        }),
                );
                let all = entity.clone();
                menu = menu.item(
                    PopupMenuItem::new(lang.tr("right.closeAll"))
                        .disabled(!can_all)
                        .on_click(move |_, _, cx| {
                            all.update(cx, |this, cx| this.close_all_tabs(cx));
                        }),
                );
                let show = entity.clone();
                menu = menu.item(PopupMenuItem::separator()).item(
                    PopupMenuItem::new(lang.tr("right.showAllTabs")).on_click(
                        move |_, window, cx| {
                            show.update(cx, |this, cx| this.open_tab_catalog(window, cx));
                        },
                    ),
                );
                menu
            });
        div()
            .id("right-tab-overflow-wrap")
            .debug_selector(|| String::from("right-tab-overflow"))
            .w(px(STRIP_OVERFLOW_W))
            .h(px(26.0))
            .flex_none()
            .child(trigger)
            .tooltip_show_delay(std::time::Duration::from_millis(500))
            .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
    }

    /// The one open tab, with the close that drops the pane back to Start.
    ///
    /// For the Files pane the chip is also the tree's own tab: when a document
    /// holds the body, the chip sits unselected and pressing it hands the body
    /// back, which is how the reference's dock moves between a pane and the
    /// documents opened out of it.
    fn right_tab_chip(
        &self,
        tab: RightTab,
        width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let lang = self.state.lang;
        let active = self.right.tab == Some(tab);
        let selected = match tab {
            // The Files pane tab is the tree's tab: when a document holds the
            // body, the file's own tab is the selected one, not this.
            RightTab::Files => active && self.files.active.is_none(),
            _ => active,
        };
        let label = lang.tr(tab.key());
        let documents = self.files.tabs.len();
        let other_tabs = documents + self.right.open_panes.len().saturating_sub(1);
        let slug = tab.slug();
        let debug = if active {
            String::from("right-tab")
        } else {
            format!("right-tab-{slug}")
        };
        let icon_color = if tab == RightTab::Files {
            // The reference paints the Files tab's folder gold; it is the one
            // pane glyph that is a file-manager icon rather than a tool mark.
            theme::warn()
        } else if selected {
            theme::text()
        } else {
            theme::muted()
        };
        let entity = cx.entity();
        div()
            .id(SharedString::from(format!("right-tab-{slug}")))
            .debug_selector(move || debug.clone())
            .w(px(width))
            .h(px(theme::RIGHT_TAB_H))
            .flex_none()
            .min_w_0()
            .px(px(8.0))
            .rounded(px(theme::RADIUS_SM))
            .when(selected, |chip| chip.bg(rgb(theme::selected_row())))
            .when(!selected, |chip| chip.cursor_pointer())
            .when(!selected, |chip| chip.hover(|s| s.bg(rgb(theme::hover()))))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .overflow_hidden()
            .child(icon(tab.glyph(), 14.0, icon_color).flex_none())
            .child(
                txt(
                    label,
                    theme::CHIP_SIZE,
                    if selected {
                        theme::text()
                    } else {
                        theme::muted()
                    },
                )
                .flex_1()
                .min_w_0()
                .text_ellipsis(),
            )
            // Only the selected tab draws a close, so an inactive tab's width
            // goes to its label instead of a hidden button. Selecting the tab
            // brings the close back, which is the reference's own behaviour.
            .when(selected, |chip| {
                chip.child(
                    div()
                        .id(SharedString::from(format!("right-tab-close-{slug}")))
                        .debug_selector(move || format!("right-tab-close-{slug}"))
                        .ml(px(2.0))
                        .px(px(3.0))
                        .flex_none()
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(theme::hover())))
                        .child(icon(IconName::X, 12.0, theme::faint()))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            cx.stop_propagation();
                            this.close_right_pane_tab(tab, cx);
                        })),
                )
            })
            .when(!selected, |chip| {
                chip.on_click(cx.listener(move |this, _event, _window, cx| {
                    if tab == RightTab::Files {
                        this.activate_pane(RightTab::Files, cx);
                        this.focus_tree(cx);
                    } else {
                        this.activate_pane(tab, cx);
                    }
                }))
            })
            // The context menu is the tab's own operations, in the order macOS
            // Finder lists them. The pane is the tab that is up here, so
            // "other tabs" means every document opened beside it.
            .context_menu(move |menu, _, _| {
                let close = entity.clone();
                let others = entity.clone();
                let all = entity.clone();
                let show = entity.clone();
                menu.item(
                    PopupMenuItem::new(lang.tr("right.close")).on_click(move |_, _, cx| {
                        close.update(cx, |this, cx| this.close_right_pane_tab(tab, cx));
                    }),
                )
                .item(
                    PopupMenuItem::new(lang.tr("right.closeOthers"))
                        .disabled(other_tabs == 0)
                        .on_click(move |_, _, cx| {
                            others.update(cx, |this, cx| this.close_other_pane_tabs(tab, cx));
                        }),
                )
                .item(
                    PopupMenuItem::new(lang.tr("right.closeAll")).on_click(move |_, _, cx| {
                        all.update(cx, |this, cx| this.close_all_tabs(cx));
                    }),
                )
                .item(PopupMenuItem::separator())
                .item(PopupMenuItem::new(lang.tr("right.showAllTabs")).on_click(
                    move |_, window, cx| {
                        show.update(cx, |this, cx| this.open_tab_catalog(window, cx));
                    },
                ))
            })
    }

    /// A small square control on the panel header.
    fn right_panel_control(
        &self,
        id: &'static str,
        mark: IconName,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .debug_selector(|| id.to_string())
            .id(id)
            .w(px(26.0))
            .h(px(26.0))
            .flex_none()
            .rounded(px(theme::RADIUS_SM))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(theme::hover())))
            .child(icon(mark, 14.0, theme::muted()))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                // The glyph is the contract: Maximise takes the column, Minimise
                // gives it back, and neither one closes the panel - the
                // collapse control beside this one does that. Both used to fold
                // the panel, so the maximise icon was lying.
                this.right.maximized = !this.right.maximized;
                cx.notify();
            }))
    }

    /// The panel's collapse control, the reference's own right-edge control.
    ///
    /// The reference labels this slot with the panel-right glyph, not an X:
    /// it collapses the whole column back behind the header's show control,
    /// and the icon says that, so a reader looking for "where did the panel
    /// go" finds the same shape they used to open it.
    fn right_panel_close(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .debug_selector(|| String::from("right-close"))
            .id("right-close")
            .w(px(26.0))
            .h(px(26.0))
            .flex_none()
            .rounded(px(theme::RADIUS_SM))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(theme::hover())))
            .child(icon(IconName::PanelRightClose, 14.0, theme::muted()))
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.right.open = false;
                this.right.catalog_open = false;
                this.browser_pane_closed();
                this.hide_terminal_view(cx);
                cx.notify();
            }))
    }

    /// The right pane's Finder-style search bar: a field and a scope control at
    /// the top of the body, above whatever pane is showing.
    ///
    /// It sits in the body rather than the tab strip so the tab strip keeps its
    /// width: a permanent field would shrink every tab and force the launcher
    /// chips into the overflow on day one. The bar is always there, on Start as
    /// well as on a pane, which is what makes searching for Browser, Extensions
    /// or a document a one-step gesture.
    fn right_search_bar(&self) -> impl IntoElement {
        let field = div()
            .id("right-search-field")
            .debug_selector(|| String::from("right-search-field"))
            .w_full()
            .min_w_0()
            .h(px(34.0))
            .px(px(9.0))
            .rounded(px(10.0))
            .border_1()
            .border_color(rgb(theme::border_soft()))
            .bg(rgb(theme::page()))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .child(icon(IconName::Search, 14.0, theme::faint()).flex_none())
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&self.right_search)
                        .appearance(false)
                        .w_full()
                        .h(px(30.0)),
                ),
            )
            .child(
                div()
                    .id("right-search-shortcut")
                    .debug_selector(|| String::from("right-search-shortcut"))
                    .flex_none()
                    .h(px(20.0))
                    .px(px(5.0))
                    .rounded(px(5.0))
                    .bg(rgb(theme::new_session_fill()))
                    .flex()
                    .items_center()
                    .justify_center()
                    .tooltip_show_delay(std::time::Duration::from_millis(500))
                    .tooltip({
                        let lang = self.state.lang;
                        move |window, cx| {
                            Tooltip::new(crate::i18n::tr(lang, "right.start.searchHint"))
                                .build(window, cx)
                        }
                    })
                    .child(txt("\u{2318}K", 9.5, theme::faint())),
            );
        hairline_bottom(
            div()
                .h(px(46.0))
                .flex_none()
                .px(px(12.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .bg(rgb(theme::sidebar_bg()))
                .child(field),
        )
        .border_color(rgb(theme::border_soft()))
    }

    /// The panel body: the Start screen, or the chosen pane.
    fn right_panel_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let __all = std::time::Instant::now();
        let body = div()
            .debug_selector(|| String::from("right-panel-body"))
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .overflow_hidden();
        let body = match self.right.tab {
            None => body.child(self.right_start(cx)),
            Some(RightTab::Files) => {
                let pane = body.child(self.files_pane(cx));
                let _ = __all;
                pane
            }
            Some(RightTab::Terminal) => body.child(self.terminal_pane(cx)),
            Some(RightTab::Diff) => body.child(self.diff_pane(cx)),
            Some(RightTab::Browser) => body.child(self.browser_pane(cx)),
            Some(RightTab::Federation) => body.child(self.federation_pane(cx)),
            Some(RightTab::Tasks) => body.child(self.tasks_pane(cx)),
            Some(RightTab::Extensions) => body.child(match &self.right_extension_panels {
                Some(view) => view.clone().into_any_element(),
                None => div()
                    .debug_selector(|| "right-extensions-empty".to_string())
                    .flex_1()
                    .px(px(theme::RIGHT_PAD * 2.0))
                    .child(txt(
                        self.state.lang.tr("right.extensions.empty"),
                        theme::CHIP_SIZE,
                        theme::faint(),
                    ))
                    .into_any_element(),
            }),
        };
        // Finder's overview sits over the body, under the strip: the tab bar
        // that opened it stays visible, so closing the catalogue is the same
        // gesture as opening it.
        body.when(self.right.catalog_open, |body| {
            body.child(self.tab_catalog_overlay(cx))
        })
    }

    /// Finder's Show All Tabs: a card for every open tab, over the body.
    ///
    /// The strip's overflow menu is a list of what did not fit; this is the
    /// deliberate overview, reached the way Finder reaches it - control-click a
    /// tab and choose Show All Tabs. Cards are sized from the panel's width so
    /// a 320px panel still gets two readable columns and a maximised one does
    /// not stretch a name across the window.
    fn tab_catalog_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        let (card_w, gap) = self.catalog_card_size();
        // Every surface the catalogue can switch to, then every document. The
        // category and search filters are applied to this one list, so a search
        // narrows the category the reader already chose instead of replacing it.
        let mut entries: Vec<(CatalogEntry, String)> = Vec::new();
        match self.right.tab {
            Some(_) => {
                for tab in RightTab::ALL {
                    entries.push((CatalogEntry::Pane(tab), lang.tr(tab.key())));
                }
            }
            None => entries.push((CatalogEntry::Start, lang.tr("right.start"))),
        }
        for (index, tab) in self.files.tabs.iter().enumerate() {
            entries.push((CatalogEntry::File(index), tab.label()));
        }
        let query = self.catalog_query_text.trim().to_lowercase();
        let shown: Vec<(CatalogEntry, String)> = entries
            .iter()
            .filter(|(entry, label)| {
                self.catalog_filter.admits(*entry)
                    && (query.is_empty() || label.to_lowercase().contains(&query))
            })
            .cloned()
            .collect();
        let search = div()
            .id("tab-catalog-search")
            .debug_selector(|| String::from("tab-catalog-search"))
            .flex_1()
            .h(px(34.0))
            .px(px(10.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(rgb(theme::border()))
            .bg(rgb(theme::page()))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            // A press in the field belongs to the field, not to the empty space
            // that closes the overview.
            .on_click(cx.listener(|_this, _event, _window, cx| {
                cx.stop_propagation();
            }))
            .child(icon(IconName::Search, 13.0, theme::faint()).flex_none())
            .child(
                Input::new(&self.catalog_query)
                    .appearance(false)
                    .w_full()
                    .h(px(32.0)),
            );

        let mut filters = div()
            .h(px(36.0))
            .flex_none()
            .px(px(16.0))
            .flex()
            .flex_row()
            .items_center();
        for filter in [
            CatalogFilter::All,
            CatalogFilter::Documents,
            CatalogFilter::Panes,
        ] {
            let active = self.catalog_filter == filter;
            let slug = filter.slug();
            filters = filters.child(
                div()
                    .id(SharedString::from(format!("tab-catalog-filter-{slug}")))
                    .debug_selector(move || format!("tab-catalog-filter-{slug}"))
                    .relative()
                    .flex_1()
                    .h(px(36.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(
                        txt(
                            lang.tr(filter.key()),
                            13.0,
                            if active {
                                theme::text()
                            } else {
                                theme::muted()
                            },
                        )
                        .font_weight(FontWeight::MEDIUM),
                    )
                    .when(active, |tab| {
                        tab.child(
                            div()
                                .absolute()
                                .bottom(px(0.0))
                                .left(px(0.0))
                                .right(px(0.0))
                                .h(px(2.0))
                                .rounded(px(1.0))
                                .bg(rgb(theme::accent())),
                        )
                    })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        cx.stop_propagation();
                        this.catalog_filter = filter;
                        cx.notify();
                    })),
            );
        }
        let mut grid = div()
            .id("tab-catalog-grid")
            .debug_selector(|| String::from("tab-catalog-grid"))
            .flex_1()
            .min_h_0()
            .w_full()
            .p(px(16.0))
            .flex()
            .flex_row()
            .flex_wrap()
            .justify_center()
            .gap(px(gap))
            .overflow_y_scroll();
        if shown.is_empty() {
            grid = grid.child(
                div()
                    .w_full()
                    .py(px(40.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.0))
                    .child(icon(IconName::Search, 20.0, theme::faint()))
                    .child(txt(lang.tr("right.tabs.noResults"), 13.0, theme::muted())),
            );
        }
        for (entry, _) in &shown {
            let card = match entry {
                CatalogEntry::Start => self.catalog_start_card(card_w, cx).into_any_element(),
                CatalogEntry::Pane(tab) => {
                    self.catalog_pane_card(*tab, card_w, cx).into_any_element()
                }
                CatalogEntry::File(index) => match self.files.tabs.get(*index) {
                    Some(tab) => self
                        .catalog_file_card(*index, tab, card_w, cx)
                        .into_any_element(),
                    None => continue,
                },
            };
            grid = grid.child(card);
        }
        // The plus card is an action, not a tab: it stays put when a search
        // narrows the catalogue to nothing.
        grid = grid.child(self.catalog_new_card(card_w, cx));

        div()
            .id("tab-catalog")
            .debug_selector(|| String::from("tab-catalog"))
            .absolute()
            .inset_0()
            .occlude()
            .bg(rgb(theme::page()))
            .flex()
            .flex_col()
            // The empty space around the cards closes the overview, the way
            // clicking away from Finder's own grid does.
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.close_tab_catalog(cx);
            }))
            .child(
                hairline_bottom(
                    div()
                        .h(px(52.0))
                        .flex_none()
                        .px(px(16.0))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0)),
                )
                .border_color(rgb(theme::border_soft()))
                .child(icon(IconName::LayoutGrid, 15.0, theme::muted()))
                .child(search)
                .child(
                    div()
                        .id("tab-catalog-close")
                        .debug_selector(|| String::from("tab-catalog-close"))
                        .w(px(26.0))
                        .h(px(26.0))
                        .flex_none()
                        .rounded(px(theme::RADIUS_SM))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(theme::hover())))
                        .child(icon(IconName::X, 13.0, theme::muted()))
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            cx.stop_propagation();
                            this.close_tab_catalog(cx);
                        })),
                ),
            )
            .child(hairline_bottom(filters).border_color(rgb(theme::border_soft())))
            .child(grid)
    }

    /// The card width and gap the catalogue grid uses at this panel width.
    fn catalog_card_size(&self) -> (f32, f32) {
        let gap = 8.0;
        let pad = 16.0;
        // The column's own 0.5px seam rule is inside the width, so leave a
        // pixel for it: a border-box card that lands exactly on the edge would
        // wrap the second column on a fractional device pixel.
        let width = (self.right_panel_w() - 1.0).max(0.0);
        let columns = (((width - pad * 2.0 + gap) / 240.0).floor() as usize).clamp(2, 5);
        let card = ((width - pad * 2.0 - gap * (columns - 1) as f32) / columns as f32).min(240.0);
        (card, gap)
    }

    /// One catalogue card, styled once so every kind reads as the same grid.
    fn catalog_card(&self, id: String, debug: String, width: f32, selected: bool) -> Stateful<Div> {
        div()
            .id(id)
            .debug_selector(move || debug.clone())
            .w(px(width))
            .h(px(56.0))
            .flex_none()
            .px(px(10.0))
            .rounded(px(theme::RADIUS_SM))
            .border_1()
            .border_color(rgb(if selected {
                theme::accent()
            } else {
                theme::border_soft()
            }))
            .bg(rgb(if selected {
                theme::selected_row()
            } else {
                theme::new_session_fill()
            }))
            .hover(|s| s.bg(rgb(theme::hover())))
            .cursor_pointer()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
    }

    /// The card for the Start screen when the pane is closed.
    fn catalog_start_card(&self, width: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        let label = self.state.lang.tr("right.start");
        self.catalog_card(
            String::from("tab-catalog-start"),
            String::from("tab-catalog-start"),
            width,
            true,
        )
        .on_click(cx.listener(|this, _event, _window, cx| {
            cx.stop_propagation();
            this.close_tab_catalog(cx);
        }))
        .child(icon(IconName::Compass, 16.0, theme::text()).flex_none())
        .child(
            txt(label, 13.0, theme::text())
                .flex_1()
                .min_w_0()
                .text_ellipsis(),
        )
    }

    /// The card for the pane that is up.
    fn catalog_pane_card(
        &self,
        tab: RightTab,
        width: f32,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let label = self.state.lang.tr(tab.key());
        self.catalog_card(
            format!("tab-catalog-pane-{}", tab.slug()),
            format!("tab-catalog-pane-{}", tab.slug()),
            width,
            true,
        )
        .on_click(cx.listener(|this, _event, _window, cx| {
            cx.stop_propagation();
            this.close_tab_catalog(cx);
        }))
        .child(icon(tab.glyph(), 16.0, theme::text()).flex_none())
        .child(
            txt(label, 13.0, theme::text())
                .flex_1()
                .min_w_0()
                .text_ellipsis(),
        )
        .child(
            div()
                .id("tab-catalog-pane-close")
                .debug_selector(|| String::from("tab-catalog-pane-close"))
                .w(px(20.0))
                .h(px(20.0))
                .flex_none()
                .rounded(px(4.0))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(rgb(theme::hover())))
                .child(icon(IconName::X, 12.0, theme::faint()))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    cx.stop_propagation();
                    this.close_tab_catalog(cx);
                    this.close_right_pane_tab(tab, cx);
                })),
        )
    }

    /// The card for one open document, with the close that drops it.
    fn catalog_file_card(
        &self,
        index: usize,
        tab: &FileTab,
        width: f32,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let name = tab.label();
        let close_name = name.clone();
        let selected = self.right.tab == Some(RightTab::Files) && self.files.active == Some(index);
        self.catalog_card(
            format!("tab-catalog-card-{index}"),
            format!("tab-catalog-card-{index}"),
            width,
            selected,
        )
        .on_click(cx.listener(move |this, _event, _window, cx| {
            cx.stop_propagation();
            this.close_tab_catalog(cx);
            this.focus_tab(index, cx);
        }))
        .child(icon(file_glyph_for_name(&name), 16.0, theme::muted()).flex_none())
        .child(
            txt(name, 13.0, theme::text())
                .flex_1()
                .min_w_0()
                .text_ellipsis(),
        )
        .child(
            div()
                .id(SharedString::from(format!("tab-catalog-close-{index}")))
                .debug_selector(move || format!("tab-catalog-close-{close_name}"))
                .w(px(20.0))
                .h(px(20.0))
                .flex_none()
                .rounded(px(4.0))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(rgb(theme::hover())))
                .child(icon(IconName::X, 12.0, theme::faint()))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    cx.stop_propagation();
                    this.close_tab(index, cx);
                })),
        )
    }

    /// The plus card: a new tab lands on the Start screen, as the strip's plus
    /// does.
    fn catalog_new_card(&self, width: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        let label = self.state.lang.tr("right.newTab");
        self.catalog_card(
            String::from("tab-catalog-new"),
            String::from("tab-catalog-new"),
            width,
            false,
        )
        .on_click(cx.listener(|this, _event, _window, cx| {
            cx.stop_propagation();
            this.close_tab_catalog(cx);
            this.show_start(cx);
        }))
        .child(icon(IconName::Plus, 16.0, theme::muted()).flex_none())
        .child(
            txt(label, 13.0, theme::muted())
                .flex_1()
                .min_w_0()
                .text_ellipsis(),
        )
    }

    /// The Tasks board: durable work grouped by state.
    /// The workspace root the active session's tasks live in.
    fn task_workspace_root(&self) -> std::path::PathBuf {
        let session = self.state.active_id.clone().unwrap_or_default();
        self.state
            .session_choice(&session)
            .workspace
            .clone()
            .and_then(|id| {
                self.state
                    .settings
                    .global
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.id == id)
                    .map(|workspace| std::path::PathBuf::from(&workspace.root))
            })
            .unwrap_or_else(|| {
                std::path::PathBuf::from(self.state.settings.active_workspace_root())
            })
    }

    /// Every workspace root a task record may live in.
    fn task_roots_for_pane(&self) -> Vec<std::path::PathBuf> {
        let mut roots = std::collections::BTreeSet::new();
        let active = self.state.settings.active_workspace_root();
        if !active.is_empty() {
            roots.insert(std::path::PathBuf::from(active));
        }
        for workspace in &self.state.settings.global.workspaces {
            if !workspace.root.is_empty() {
                roots.insert(std::path::PathBuf::from(&workspace.root));
            }
        }
        roots.into_iter().collect()
    }

    /// The Tasks tab: the run graph by default, the store list on request.
    fn tasks_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.graph_board {
            self.tasks_list(cx)
        } else if self.graph_tree {
            self.task_graph_pane(cx)
        } else {
            self.task_graph_list(cx)
        }
    }

    /// The List projection of the same work-unit graph. It is deliberately not
    /// the durable task board: a session with subagents but no task records
    /// still has work units, and this is where they are listed.
    fn task_graph_list(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::ui::task_graph::{GraphStatus, NodeKind};
        let lang = self.state.lang;
        let session = self.state.active_id.clone().unwrap_or_default();
        let title = self
            .state
            .store
            .session(&session)
            .and_then(|session| {
                session
                    .messages
                    .iter()
                    .rev()
                    .find(|message| message.role == crate::model::Role::User)
                    .map(|message| message.text.clone())
            })
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| {
                self.state
                    .store
                    .session(&session)
                    .map(|session| session.title.clone())
                    .unwrap_or_default()
            });
        let title: String = title.chars().take(64).collect();
        let status = if self.state.streams.contains_key(&session) {
            GraphStatus::Running
        } else {
            GraphStatus::Done
        };
        let runs = self.state.subagents.runs();
        let workspace = self.task_workspace_root();
        let tasks = {
            use crate::extension::tasks::TasksStore as _;
            let mut cache = self.right.tasks.borrow_mut();
            if cache.is_none() {
                *cache = Some(
                    crate::extension::tasks::FileTasksStore
                        .list(&workspace)
                        .unwrap_or_default(),
                );
            }
            cache.clone().unwrap_or_default()
        };
        let peers: Vec<crate::ui::task_graph::PeerNode> = self
            .right
            .federation
            .delegations
            .iter()
            .map(|row| crate::ui::task_graph::PeerNode {
                id: row.id.clone(),
                title: row.objective.clone(),
                target: row.target.clone(),
                status: peer_graph_status(row.state),
            })
            .collect();
        let graph = crate::ui::task_graph::layout_with_peers(
            &session, &title, status, &tasks, &runs, &peers, None,
        );
        let delegated = graph
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, NodeKind::Subagent | NodeKind::Peer))
            .count();
        let meta = format!(
            "1 {} \u{00B7} {} {} \u{00B7} {} {}",
            lang.tr("right.tasks.mainTask"),
            delegated,
            lang.tr("right.tasks.delegatedTasks"),
            graph.nodes.len(),
            lang.tr("right.tasks.totalNodes"),
        );
        let mut rows = div()
            .id("task-list-scroll")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(10.0))
            .py(px(8.0));
        for node in &graph.nodes {
            let id = node.id.clone();
            let click_id = node.id.clone();
            let kind = match node.kind {
                NodeKind::Main => lang.tr("right.tasks.mainAgent"),
                NodeKind::Subagent => lang.tr("right.tasks.subagent"),
                NodeKind::Task => lang.tr("right.tasks.task"),
                NodeKind::Peer => lang.tr("right.tasks.peer"),
            };
            let ink = match node.status {
                GraphStatus::Failed => theme::error(),
                GraphStatus::Blocked | GraphStatus::Waiting => theme::warn(),
                GraphStatus::Running => theme::accent(),
                GraphStatus::Cancelled => theme::faint(),
                _ => theme::success(),
            };
            let selected = self.graph_selected.as_deref() == Some(id.as_str());
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("task-list-row-{id}")))
                    .debug_selector(move || format!("task-list-row-{id}"))
                    .h(px(54.0))
                    .px(px(8.0))
                    .rounded(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .when(selected, |row| row.bg(rgb(theme::soft(theme::accent()))))
                    .when(!selected, |row| row.hover(|s| s.bg(rgb(theme::hover()))))
                    .cursor_pointer()
                    .child(div().w(px(6.0)).h(px(6.0)).rounded_full().bg(rgb(ink)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                txt(node.title.clone(), 12.0, theme::text())
                                    .font_weight(FontWeight::MEDIUM)
                                    .truncate(),
                            )
                            .child(
                                txt(
                                    format!(
                                        "{} \u{00B7} {} \u{00B7} {} {}",
                                        kind,
                                        lang.tr(node.status.key()),
                                        node.steps,
                                        lang.tr("right.tasks.steps")
                                    ),
                                    10.0,
                                    theme::muted(),
                                )
                                .truncate(),
                            ),
                    )
                    .child(txt(lang.tr(node.status.key()), 10.0, ink))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.graph_selected = Some(click_id.clone());
                        this.graph_tree = true;
                        this.graph_board = false;
                        cx.notify();
                    })),
            );
        }
        div()
            .debug_selector(|| "task-graph-list-view".to_string())
            .flex()
            .flex_col()
            .h_full()
            .child(
                div()
                    .px(px(12.0))
                    .pt(px(12.0))
                    .pb(px(8.0))
                    .flex()
                    .flex_col()
                    .gap(px(5.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                txt(title, 13.0, theme::text())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .flex_1()
                                    .min_w_0()
                                    .truncate(),
                            )
                            .child(
                                div()
                                    .id("task-view-board")
                                    .debug_selector(|| "task-view-board".to_string())
                                    .h(px(24.0))
                                    .px(px(8.0))
                                    .rounded(px(7.0))
                                    .border_1()
                                    .border_color(rgb(theme::border_soft()))
                                    .flex()
                                    .items_center()
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(theme::hover())))
                                    .child(txt(lang.tr("right.tasks.board"), 10.0, theme::muted()))
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.graph_board = true;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(txt(meta, 10.0, theme::faint())),
            )
            .child(
                div()
                    .px(px(12.0))
                    .pb(px(7.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .id("task-graph-tree")
                            .debug_selector(|| "task-graph-tree".to_string())
                            .h(px(26.0))
                            .px(px(9.0))
                            .rounded(px(7.0))
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(theme::hover())))
                            .child(txt(
                                format!("\u{2191} {}", lang.tr("right.tasks.tree")),
                                11.5,
                                theme::faint(),
                            ))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.graph_tree = true;
                                this.graph_board = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .h(px(26.0))
                            .px(px(9.0))
                            .rounded(px(7.0))
                            .border_1()
                            .border_color(rgb(theme::border_soft()))
                            .bg(rgb(theme::page()))
                            .flex()
                            .items_center()
                            .child(txt(lang.tr("right.tasks.list"), 11.5, theme::text())),
                    ),
            )
            .child(rows)
            .into_any_element()
    }

    /// The DSH-style task graph: a main-agent card over its subagent children.
    fn task_graph_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let session = self.state.active_id.clone().unwrap_or_default();
        let title = self
            .state
            .store
            .session(&session)
            .and_then(|session| {
                session
                    .messages
                    .iter()
                    .rev()
                    .find(|message| message.role == crate::model::Role::User)
                    .map(|message| message.text.clone())
            })
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| {
                self.state
                    .store
                    .session(&session)
                    .map(|session| session.title.clone())
                    .unwrap_or_default()
            });
        let title: String = title.chars().take(64).collect();
        let status = if self.state.streams.contains_key(&session) {
            crate::ui::task_graph::GraphStatus::Running
        } else {
            crate::ui::task_graph::GraphStatus::Done
        };
        let runs = self.state.subagents.runs();
        // Tasks live beside the session's own workspace, the same root the
        // task tools wrote them to. The cache is refreshed on tab open and at
        // turn end, never per frame.
        let workspace = self.task_workspace_root();
        let tasks = {
            use crate::extension::tasks::TasksStore as _;
            let mut cache = self.right.tasks.borrow_mut();
            if cache.is_none() {
                *cache = Some(
                    crate::extension::tasks::FileTasksStore
                        .list(&workspace)
                        .unwrap_or_default(),
                );
            }
            cache.clone().unwrap_or_default()
        };
        // The graph fits the pane by default: a wide tree wraps into staggered
        // rows first, then scales down only if the rows are still too wide.
        let available_w = (self.right_panel_w() - 24.0).max(240.0);
        // The inspector is part of the pane, not a popover: the main agent is
        // selected until the reader picks a child, so the graph always leaves
        // it its measured band.
        let available_h = (self.viewport_h - 194.0 - 104.0).max(220.0);
        let peers: Vec<crate::ui::task_graph::PeerNode> = self
            .right
            .federation
            .delegations
            .iter()
            .map(|row| crate::ui::task_graph::PeerNode {
                id: row.id.clone(),
                title: row.objective.clone(),
                target: row.target.clone(),
                status: peer_graph_status(row.state),
            })
            .collect();
        let graph = crate::ui::task_graph::layout_with_peers(
            &session,
            &title,
            status,
            &tasks,
            &runs,
            &peers,
            Some(available_w),
        );
        let fit = crate::ui::task_graph::fit_zoom(
            graph.width + crate::ui::task_graph::MARGIN * 2.0,
            graph.height + crate::ui::task_graph::MARGIN * 2.0,
            available_w,
            available_h,
        );
        let zoom = if self.right.graph_fit {
            fit
        } else {
            self.graph_zoom
        };
        let pan = self.graph_pan;
        let offset_x = ((available_w - graph.width * zoom) / 2.0).max(0.0);
        let offset_y = ((available_h - graph.height * zoom) / 2.0).max(0.0);
        let scale_x = |x: f32| crate::ui::task_graph::MARGIN + x * zoom + pan.0 + offset_x;
        let scale_y = |y: f32| crate::ui::task_graph::MARGIN + y * zoom + pan.1 + offset_y;
        let card_w = crate::ui::task_graph::CARD_W * zoom;
        let card_h = crate::ui::task_graph::CARD_H * zoom;
        let content_w = available_w;
        let content_h =
            (graph.height * zoom + crate::ui::task_graph::MARGIN * 2.0 + offset_y * 2.0)
                .max(available_h);

        let mut edges: Vec<(f32, f32, f32, f32)> = Vec::new();
        for (parent, child) in &graph.edges {
            let (Some(parent), Some(child)) = (graph.nodes.get(*parent), graph.nodes.get(*child))
            else {
                continue;
            };
            edges.push((
                scale_x(parent.x) + card_w / 2.0,
                scale_y(parent.y) + card_h,
                scale_x(child.x) + card_w / 2.0,
                scale_y(child.y),
            ));
        }
        let canvas_edges = canvas(
            move |bounds, _window, _cx| bounds,
            move |state, _bounds, window, _cx| {
                let base = state.origin;
                for (sx, sy, ex, ey) in &edges {
                    let start = (base.x + px(*sx), base.y + px(*sy));
                    let end = (base.x + px(*ex), base.y + px(*ey));
                    // A smooth S: leave the parent straight down, travel
                    // sideways only in the gap between levels, and enter the
                    // child straight from above, so the line never crosses a card.
                    let gap = (end.1 - start.1).max(px(16.0));
                    let control_a = (start.0, start.1 + gap * 0.45);
                    let control_b = (end.0, end.1 - gap * 0.45);
                    let mut path = PathBuilder::stroke(px(1.5));
                    path.move_to(point(start.0, start.1));
                    for step in 1..=24 {
                        let t = step as f32 / 24.0;
                        let inv = 1.0 - t;
                        let x = inv * inv * inv * start.0
                            + 3.0 * inv * inv * t * control_a.0
                            + 3.0 * inv * t * t * control_b.0
                            + t * t * t * end.0;
                        let y = inv * inv * inv * start.1
                            + 3.0 * inv * inv * t * control_a.1
                            + 3.0 * inv * t * t * control_b.1
                            + t * t * t * end.1;
                        path.line_to(point(x, y));
                    }
                    if let Ok(path) = path.build() {
                        window.paint_path(path, rgb(theme::border()));
                    }
                }
            },
        );

        let subagent_count = graph
            .nodes
            .iter()
            .filter(|node| node.kind == crate::ui::task_graph::NodeKind::Subagent)
            .count();
        let peer_count = graph
            .nodes
            .iter()
            .filter(|node| node.kind == crate::ui::task_graph::NodeKind::Peer)
            .count();
        let active_subagents = graph
            .nodes
            .iter()
            .filter(|node| {
                node.kind == crate::ui::task_graph::NodeKind::Subagent
                    && node.status == crate::ui::task_graph::GraphStatus::Running
            })
            .count();
        let total_nodes = graph.nodes.len();
        let meta = format!(
            "1 {} \u{00B7} {} {} \u{00B7} {} {}",
            lang.tr("right.tasks.mainTask"),
            subagent_count + peer_count,
            lang.tr("right.tasks.delegatedTasks"),
            total_nodes,
            lang.tr("right.tasks.totalNodes"),
        );
        let header = div()
            .debug_selector(|| "task-graph-header".to_string())
            .px(px(12.0))
            .pt(px(12.0))
            .pb(px(8.0))
            .flex()
            .flex_col()
            .gap(px(5.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        txt(title.clone(), 13.0, theme::text())
                            .font_weight(FontWeight::SEMIBOLD)
                            .flex_1()
                            .min_w_0()
                            .truncate(),
                    )
                    .when(active_subagents > 0, |row| {
                        row.child(
                            div()
                                .h(px(20.0))
                                .px(px(7.0))
                                .rounded(px(999.0))
                                .bg(rgb(theme::soft(theme::success())))
                                .flex()
                                .items_center()
                                .gap(px(5.0))
                                .child(
                                    div()
                                        .w(px(6.0))
                                        .h(px(6.0))
                                        .rounded_full()
                                        .bg(rgb(theme::success())),
                                )
                                .child(txt(
                                    format!(
                                        "{} {}",
                                        active_subagents,
                                        lang.tr("right.tasks.subagents")
                                    ),
                                    9.5,
                                    theme::success(),
                                )),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(txt(meta, 10.0, theme::faint()).flex_1().truncate())
                    .child(
                        div()
                            .id("task-view-board")
                            .debug_selector(|| "task-view-board".to_string())
                            .h(px(22.0))
                            .px(px(8.0))
                            .rounded(px(7.0))
                            .border_1()
                            .border_color(rgb(theme::border_soft()))
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(theme::hover())))
                            .child(txt(lang.tr("right.tasks.board"), 9.5, theme::muted()))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
                                    this.graph_board = true;
                                    this.graph_tree = false;
                                    cx.notify();
                                }),
                            )
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.graph_board = true;
                                this.graph_tree = false;
                                cx.notify();
                            })),
                    ),
            );
        let mut cards: Vec<AnyElement> = Vec::with_capacity(graph.nodes.len());
        for node in &graph.nodes {
            cards.push(self.task_graph_node(
                node,
                zoom,
                (pan.0 + offset_x, pan.1 + offset_y),
                lang,
                cx,
            ));
        }
        let tree_label = lang.tr("right.tasks.tree");
        let list_label = lang.tr("right.tasks.list");
        let tree_segment = {
            let on = self.graph_tree;
            div()
                .id("task-graph-tree")
                .debug_selector(|| "task-graph-tree".to_string())
                .cursor_pointer()
                .h(px(26.0))
                .px(px(9.0))
                .rounded(px(7.0))
                .flex()
                .items_center()
                .gap(px(4.0))
                .when(on, |cell| {
                    cell.bg(rgb(theme::page()))
                        .border_1()
                        .border_color(rgb(theme::border_soft()))
                })
                .child(txt(
                    format!("\u{2191} {tree_label}"),
                    11.5,
                    if on { theme::text() } else { theme::faint() },
                ))
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.graph_tree = true;
                    cx.notify();
                }))
        };
        let list_segment = {
            let on = !self.graph_tree;
            div()
                .id("task-graph-list")
                .debug_selector(|| "task-graph-list".to_string())
                .cursor_pointer()
                .h(px(26.0))
                .px(px(9.0))
                .rounded(px(7.0))
                .flex()
                .items_center()
                .gap(px(4.0))
                .when(on, |cell| {
                    cell.bg(rgb(theme::page()))
                        .border_1()
                        .border_color(rgb(theme::border_soft()))
                })
                .child(txt(
                    list_label,
                    11.5,
                    if on { theme::text() } else { theme::faint() },
                ))
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.graph_tree = false;
                    cx.notify();
                }))
        };
        let mode = div()
            .id("task-graph-mode")
            .debug_selector(|| "task-graph-mode".to_string())
            .flex()
            .items_center()
            .gap(px(2.0))
            .p(px(2.0))
            .h(px(32.0))
            .rounded(px(9.0))
            .bg(rgb(theme::new_session_fill()))
            .child(tree_segment)
            .child(list_segment);
        let zoom_out = zoom_button("task-graph-zoom-out", "\u{2212}", cx, move |this| {
            let base = if this.right.graph_fit {
                fit
            } else {
                this.graph_zoom
            };
            this.right.graph_fit = false;
            this.graph_zoom = (base - 0.1).max(0.4);
        });
        let zoom_in = zoom_button("task-graph-zoom-in", "+", cx, move |this| {
            let base = if this.right.graph_fit {
                fit
            } else {
                this.graph_zoom
            };
            this.right.graph_fit = false;
            this.graph_zoom = (base + 0.1).min(2.0);
        });
        let zoom_fit = zoom_button("task-graph-zoom-reset", "\u{2922}", cx, |this| {
            this.right.graph_fit = true;
            this.graph_zoom = 1.0;
        });
        let percent = txt(
            format!("{}%", (zoom * 100.0).round() as u32),
            12.0,
            theme::muted(),
        )
        .w(px(44.0))
        .text_align(gpui::TextAlign::Center);
        // The reference's Live chip is a state, not a filter: the tree is the
        // run's own event projection, so it cannot show a stale snapshot.
        let live_chip = div()
            .h(px(26.0))
            .px(px(8.0))
            .rounded(px(999.0))
            .bg(rgb(theme::soft(theme::success())))
            .flex()
            .items_center()
            .gap(px(5.0))
            .child(
                div()
                    .w(px(6.0))
                    .h(px(6.0))
                    .rounded_full()
                    .bg(rgb(theme::success())),
            )
            .child(txt(lang.tr("right.tasks.live"), 10.0, theme::success()));
        let left = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(mode)
            .child(live_chip);
        let toolbar = div()
            .flex()
            .items_center()
            .justify_between()
            .px(px(12.0))
            .py(px(7.0))
            .border_b_1()
            .border_color(rgb(theme::border_soft()))
            .child(left)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(zoom_out)
                    .child(percent)
                    .child(zoom_in)
                    .child(zoom_fit),
            );
        let selected = self
            .graph_selected
            .as_deref()
            .and_then(|id| graph.nodes.iter().find(|node| node.id == id))
            .or_else(|| graph.nodes.first());
        let footer = div()
            .h(px(34.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .border_t_1()
            .border_color(rgb(theme::border_soft()))
            .child(
                div()
                    .w(px(6.0))
                    .h(px(6.0))
                    .rounded_full()
                    .bg(rgb(theme::success())),
            )
            .child(txt(lang.tr("right.tasks.liveTree"), 10.0, theme::muted()))
            .child(div().flex_1())
            .child(
                div()
                    .id("task-graph-fit-view")
                    .debug_selector(|| "task-graph-fit-view".to_string())
                    .h(px(24.0))
                    .px(px(9.0))
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(rgb(theme::border_soft()))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(theme::hover())))
                    .child(txt(lang.tr("right.tasks.fitView"), 10.0, theme::muted()))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.right.graph_fit = true;
                        this.graph_zoom = 1.0;
                        this.graph_pan = (0.0, 0.0);
                        cx.notify();
                    })),
            );
        let inspector = selected.map(|node| self.task_graph_inspector(node, lang));
        div()
            .flex()
            .flex_col()
            .h_full()
            .child(header)
            .child(toolbar)
            .child(
                div()
                    .id("task-graph-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_scroll()
                    .child(
                        div()
                            .id("task-graph-canvas")
                            .debug_selector(|| "task-graph-canvas".to_string())
                            .relative()
                            .w(px(content_w))
                            .h(px(content_h))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                                    this.graph_pan_drag = Some((
                                        this.graph_pan,
                                        (event.position.x.as_f32(), event.position.y.as_f32()),
                                    ));
                                    cx.notify();
                                }),
                            )
                            .on_mouse_move(cx.listener(
                                |this, event: &MouseMoveEvent, _window, cx| {
                                    let Some((pan, start)) = this.graph_pan_drag else {
                                        return;
                                    };
                                    this.graph_pan = (
                                        pan.0 + event.position.x.as_f32() - start.0,
                                        pan.1 + event.position.y.as_f32() - start.1,
                                    );
                                    cx.notify();
                                },
                            ))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                                    this.graph_pan_drag = None;
                                    cx.notify();
                                }),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left(px(0.0))
                                    .top(px(0.0))
                                    .right(px(0.0))
                                    .bottom(px(0.0))
                                    .child(canvas_edges),
                            )
                            .children(cards),
                    ),
            )
            .child(footer)
            .when_some(inspector, |pane, inspector| pane.child(inspector))
            .into_any_element()
    }

    /// The selected work unit's bottom inspector: identity on the left,
    /// delegation facts on the right, with the full prompt/result as the note.
    fn task_graph_inspector(
        &self,
        node: &crate::ui::task_graph::GraphNode,
        lang: crate::i18n::Lang,
    ) -> AnyElement {
        let kind = match node.kind {
            crate::ui::task_graph::NodeKind::Main => lang.tr("right.tasks.mainAgent"),
            crate::ui::task_graph::NodeKind::Subagent => lang.tr("right.tasks.subagent"),
            crate::ui::task_graph::NodeKind::Task => lang.tr("right.tasks.task"),
            crate::ui::task_graph::NodeKind::Peer => lang.tr("right.tasks.peer"),
        };
        let status = node.status;
        let status_ink = match status {
            crate::ui::task_graph::GraphStatus::Failed => theme::error(),
            crate::ui::task_graph::GraphStatus::Blocked
            | crate::ui::task_graph::GraphStatus::Waiting => theme::warn(),
            crate::ui::task_graph::GraphStatus::Running => theme::accent(),
            crate::ui::task_graph::GraphStatus::Cancelled => theme::faint(),
            _ => theme::success(),
        };
        div()
            .debug_selector(|| "task-graph-inspector".to_string())
            .mx(px(8.0))
            .mb(px(8.0))
            .p(px(10.0))
            .rounded(px(10.0))
            .border_1()
            .border_color(rgb(theme::border_soft()))
            .bg(rgb(theme::page()))
            .flex()
            .items_center()
            .gap(px(12.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(txt(node.title.clone(), 12.0, theme::text()).truncate())
                    .child(txt(
                        format!(
                            "{} · {} · {} {}",
                            kind,
                            lang.tr(status.key()),
                            node.steps,
                            lang.tr("right.tasks.steps")
                        ),
                        10.0,
                        theme::muted(),
                    )),
            )
            .child(
                div()
                    .w(px(180.0))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(txt(lang.tr("right.tasks.delegation"), 9.0, theme::faint()))
                    .child(
                        txt(node.note.clone().unwrap_or_default(), 10.0, theme::muted()).truncate(),
                    ),
            )
            .child(
                div()
                    .w(px(6.0))
                    .h(px(6.0))
                    .rounded_full()
                    .bg(rgb(status_ink)),
            )
            .into_any_element()
    }

    /// One graph card: the kind chip, the title and the status row.
    fn task_graph_node(
        &self,
        node: &crate::ui::task_graph::GraphNode,
        zoom: f32,
        pan: (f32, f32),
        lang: crate::i18n::Lang,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::ui::task_graph::{GraphStatus, NodeKind, CARD_H, CARD_W, MARGIN};
        let (kind_key, kind_ink) = match node.kind {
            NodeKind::Main => ("right.tasks.mainAgent", theme::accent()),
            NodeKind::Subagent => ("right.tasks.subagent", theme::accent()),
            NodeKind::Task => ("right.tasks.task", theme::muted()),
            NodeKind::Peer => ("right.tasks.peer", theme::warn()),
        };
        let (status_label, status_color) = match node.status {
            GraphStatus::Running => (lang.tr(node.status.key()), theme::accent()),
            GraphStatus::Queued | GraphStatus::Scheduled => {
                (lang.tr(node.status.key()), theme::muted())
            }
            GraphStatus::Waiting => (lang.tr(node.status.key()), theme::warn()),
            GraphStatus::Blocked => (lang.tr(node.status.key()), theme::warn()),
            GraphStatus::Done => (lang.tr(node.status.key()), theme::success()),
            GraphStatus::Failed => (lang.tr(node.status.key()), theme::error()),
            GraphStatus::Cancelled => (lang.tr(node.status.key()), theme::faint()),
        };
        let selected = self
            .graph_selected
            .as_deref()
            .map_or(node.kind == NodeKind::Main, |id| id == node.id);
        let x = MARGIN + node.x * zoom + pan.0;
        let y = MARGIN + node.y * zoom + pan.1;
        let id = format!("task-graph-node-{}", node.id);
        let click_id = node.id.clone();
        let subtitle = node
            .note
            .as_deref()
            .map(|note| {
                let flat = note.split_whitespace().collect::<Vec<_>>().join(" ");
                if flat.chars().count() > 72 {
                    format!("{}...", flat.chars().take(72).collect::<String>())
                } else {
                    flat
                }
            })
            .or_else(|| node.subtitle.map(|key| lang.tr(key)));
        let header = div()
            .flex()
            .items_center()
            .gap(px(5.0))
            .w_full()
            .min_w_0()
            .child(
                div()
                    .h(px(18.0))
                    .px(px(6.0))
                    .rounded(px(5.0))
                    .bg(rgb(theme::soft(kind_ink)))
                    .flex()
                    .items_center()
                    .child(txt(lang.tr(kind_key).to_uppercase(), 8.5, kind_ink)),
            )
            .child(div().flex_1())
            .child(
                div()
                    .h(px(18.0))
                    .px(px(6.0))
                    .rounded(px(999.0))
                    .bg(rgb(theme::soft(status_color)))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        div()
                            .w(px(5.0))
                            .h(px(5.0))
                            .rounded_full()
                            .bg(rgb(status_color)),
                    )
                    .child(txt(status_label.clone(), 9.0, status_color)),
            );
        div()
            .id(SharedString::from(format!("task-graph-card-{click_id}")))
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(CARD_W * zoom))
            .h(px(CARD_H * zoom))
            .debug_selector(move || id.clone())
            .rounded(px(12.0))
            .border_1()
            .border_color(rgb(if selected || node.kind == NodeKind::Main {
                theme::accent()
            } else {
                theme::border_soft()
            }))
            .bg(rgb(if selected {
                theme::soft(theme::accent())
            } else {
                theme::page()
            }))
            .cursor_pointer()
            .hover(|style| style.border_color(rgb(theme::accent())))
            .flex()
            .flex_col()
            .justify_between()
            .p(px(10.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .min_w_0()
                    .gap(px(5.0))
                    .child(header)
                    .child(
                        txt(node.title.clone(), 13.0, theme::text())
                            .font_weight(FontWeight::SEMIBOLD)
                            .line_height(px(18.0))
                            .line_clamp(2),
                    )
                    .when_some(subtitle, |column, subtitle| {
                        column.child(
                            txt(subtitle, 10.0, theme::muted())
                                .line_height(px(15.0))
                                .truncate(),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .border_t_1()
                    .border_color(rgb(theme::border_soft()))
                    .pt(px(6.0))
                    .child(
                        div()
                            .w(px(6.0))
                            .h(px(6.0))
                            .rounded_full()
                            .bg(rgb(status_color)),
                    )
                    .child(txt(status_label, 10.0, theme::muted()))
                    .child(div().flex_1())
                    .child(txt(
                        format!("{} {}", node.steps, lang.tr("right.tasks.steps")),
                        9.5,
                        theme::faint(),
                    )),
            )
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.graph_selected = Some(click_id.clone());
                cx.notify();
            }))
            .into_any_element()
    }

    fn tasks_list(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::extension::tasks::{FileTasksStore, TasksStore as _};
        let lang = self.state.lang;
        let root = self.task_workspace_root();
        let tasks = {
            let mut cache = self.right.tasks.borrow_mut();
            if cache.is_none() {
                *cache = Some(FileTasksStore.list(&root).unwrap_or_default());
            }
            cache.clone().unwrap_or_default()
        };
        let board_header = div()
            .px(px(12.0))
            .pt(px(10.0))
            .pb(px(6.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                txt(lang.tr("right.tasks.board"), 12.0, theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .flex_1(),
            )
            .child(
                div()
                    .id("task-view-list")
                    .debug_selector(|| "task-view-list".to_string())
                    .h(px(24.0))
                    .px(px(8.0))
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(rgb(theme::border_soft()))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(theme::hover())))
                    .child(txt(lang.tr("right.tasks.list"), 10.0, theme::muted()))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.graph_board = false;
                        this.graph_tree = false;
                        cx.notify();
                    })),
            );

        let board = crate::extension::tasks::board(&tasks);
        let mut list = div()
            .id("tasks-scroll")
            .flex()
            .flex_col()
            .gap(px(2.0))
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .px(px(12.0))
            .py(px(12.0));
        let mut any = false;
        for section in &board.sections {
            if section.rows.is_empty() {
                continue;
            }
            any = true;
            if section.kind == crate::extension::tasks::BoardSectionKind::History {
                let open = self.right.history_open;
                let label = format!(
                    "{} ({})",
                    lang.tr(Self::task_section_key(section.kind)),
                    section.rows.len()
                );
                let header = div()
                    .id("tasks-history-header")
                    .debug_selector(|| "tasks-history-header".to_string())
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(8.0))
                    .pt(px(10.0))
                    .pb(px(4.0))
                    .cursor_pointer()
                    .hover(|style| style.opacity(0.8))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.right.history_open = !this.right.history_open;
                        cx.notify();
                    }))
                    .child(txt(label, 11.0, theme::faint()))
                    .child(txt(
                        if open { "\u{25BE}" } else { "\u{25B8}" },
                        11.0,
                        theme::faint(),
                    ));
                let clear = div()
                    .id("tasks-clear-completed")
                    .debug_selector(|| "tasks-clear-completed".to_string())
                    .px(px(8.0))
                    .pt(px(10.0))
                    .pb(px(4.0))
                    .cursor_pointer()
                    .hover(|style| style.opacity(0.8))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.clear_completed_tasks();
                        cx.notify();
                    }))
                    .child(txt(
                        lang.tr("right.tasks.clearCompleted"),
                        11.0,
                        theme::faint(),
                    ));
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(header)
                        .child(clear),
                );
                if open {
                    for row in &section.rows {
                        list = list.child(self.task_row(row, lang, cx));
                    }
                }
                continue;
            }
            list = list.child(
                txt(
                    lang.tr(Self::task_section_key(section.kind)),
                    11.0,
                    theme::faint(),
                )
                .px(px(8.0))
                .pt(px(10.0))
                .pb(px(4.0)),
            );
            for row in &section.rows {
                list = list.child(self.task_row(row, lang, cx));
            }
        }
        if !any {
            // The board has no durable records, but the session still has work
            // units. Do not strand the reader on an empty card: show the same
            // List projection the Tree/List control offers.
            return self.task_graph_list(cx);
        }
        div()
            .flex()
            .flex_col()
            .h_full()
            .child(board_header)
            .child(list)
            .into_any_element()
    }

    /// One board row: a status dot, the title, and the tier and host it runs
    /// on. The row is the unit the pane exists for, so it carries its id for
    /// the click tests.
    /// One board row: the summary, plus the detail and actions when expanded.
    fn task_row(
        &self,
        row: &crate::extension::tasks::BoardRow,
        lang: crate::i18n::Lang,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::extension::tasks::{Host, Tier};
        let tier = match row.tier {
            Tier::Turn => "right.tasks.tierTurn",
            Tier::Subagent => "right.tasks.tierSubagent",
            Tier::Job => "right.tasks.tierJob",
            Tier::Delegation => "right.tasks.tierDelegation",
            Tier::PeerTask => "right.tasks.tierPeerTask",
        };
        let host = match &row.host {
            Host::Local => lang.tr("right.tasks.local").to_string(),
            Host::Peer { node } => format!("{}: {node}", lang.tr("right.tasks.peer")),
        };
        let meta = format!("{} \u{00B7} {}", lang.tr(tier), host);
        let id = format!("task-row-{}", row.id);
        let expanded = self.right.expanded.contains(&row.id);
        let toggle = row.id.clone();
        let summary = div()
            .id(SharedString::from(id.clone()))
            .debug_selector(move || id.clone())
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .min_h(px(40.0))
            .px(px(8.0))
            .rounded(px(8.0))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(theme::hover())))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                if !this.right.expanded.insert(toggle.clone()) {
                    this.right.expanded.remove(&toggle);
                }
                cx.notify();
            }))
            .child(
                div()
                    .w(px(6.0))
                    .h(px(6.0))
                    .flex_none()
                    .rounded_full()
                    .bg(rgb(Self::task_status_color(row.status))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .flex_1()
                    .gap(px(2.0))
                    .child(txt(row.title.clone(), 13.0, theme::text()).truncate())
                    .child(txt(meta, 11.0, theme::faint()).truncate()),
            );
        let mut column = div().flex().flex_col();
        column = column.child(summary);
        if expanded {
            column = column.child(self.task_detail(row, lang, cx));
        }
        column.into_any_element()
    }

    /// The expanded detail: intent, trigger, budget, attempts and actions.
    fn task_detail(
        &self,
        row: &crate::extension::tasks::BoardRow,
        lang: crate::i18n::Lang,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::extension::tasks::{FileTasksStore, Host, TaskStatus, TasksStore, Tier};
        let root = self.task_workspace_root();
        let record = FileTasksStore.load(&root, &row.id).ok().flatten();
        let mut detail = div()
            .debug_selector({
                let id = format!("task-detail-{}", row.id);
                move || id.clone()
            })
            .flex()
            .flex_col()
            .gap(px(4.0))
            .px(px(14.0))
            .pb(px(8.0));
        if let Some(record) = &record {
            detail = detail
                .child(txt(lang.tr("right.tasks.intent"), 11.0, theme::faint()).pt(px(4.0)))
                .child(txt(record.intent.clone(), 12.0, theme::muted()));
            if let Some(next) = record.trigger.next_fire_ms() {
                detail = detail.child(txt(
                    format!("{}: {}", lang.tr("right.tasks.next"), next),
                    11.0,
                    theme::faint(),
                ));
            }
            if !record.attempts.is_empty() {
                detail = detail
                    .child(txt(lang.tr("right.tasks.attempts"), 11.0, theme::faint()).pt(px(4.0)));
                for attempt in &record.attempts {
                    let result = attempt
                        .result
                        .as_deref()
                        .map(|text| text.chars().take(160).collect::<String>())
                        .unwrap_or_default();
                    detail = detail.child(txt(
                        format!(
                            "{} \u{00B7} {} {}",
                            attempt.id,
                            format!("{:?}", attempt.status).to_lowercase(),
                            result
                        ),
                        11.0,
                        theme::muted(),
                    ));
                    if let Some(change_set) = &attempt.change_set_id {
                        let change_set = change_set.clone();
                        let review_id = format!("task-review-{}", attempt.id);
                        detail = detail.child(
                            div()
                                .id(SharedString::from(review_id.clone()))
                                .debug_selector(move || review_id.clone())
                                .px(px(8.0))
                                .py(px(3.0))
                                .rounded(px(6.0))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(theme::hover())))
                                .on_click(cx.listener(move |this, _event, _window, cx| {
                                    this.review_requested = Some(change_set.clone());
                                    this.state.tab = crate::state::Tab::Review;
                                    cx.notify();
                                }))
                                .child(txt(lang.tr("right.tasks.review"), 10.5, theme::accent())),
                        );
                    }
                }
            }
        }
        let id = row.id.clone();
        let can_stop = matches!(row.status, TaskStatus::Running) && matches!(row.host, Host::Local);
        let can_retry = matches!(row.tier, Tier::Turn)
            && matches!(row.host, Host::Local)
            && !matches!(
                row.status,
                TaskStatus::Running
                    | TaskStatus::Queued
                    | TaskStatus::Scheduled
                    | TaskStatus::Waiting
            );
        let mut actions = div().flex().items_center().gap(px(6.0)).pt(px(4.0));
        if can_retry {
            let retry_id = format!("task-retry-{}", row.id);
            let retry_task = row.id.clone();
            actions = actions.child(
                div()
                    .id(SharedString::from(retry_id.clone()))
                    .debug_selector(move || retry_id.clone())
                    .px(px(8.0))
                    .py(px(3.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(rgb(theme::border_soft()))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(theme::hover())))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.retry_task_turn(&retry_task);
                        cx.notify();
                    }))
                    .child(txt(lang.tr("right.tasks.retry"), 11.0, theme::text())),
            );
        }
        if can_stop {
            let stop_id = format!("task-stop-{}", row.id);
            actions = actions.child(
                div()
                    .id(SharedString::from(stop_id.clone()))
                    .debug_selector(move || stop_id.clone())
                    .px(px(8.0))
                    .py(px(3.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(rgb(theme::border_soft()))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(theme::hover())))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.stop_task_local(&id);
                        cx.notify();
                    }))
                    .child(txt(lang.tr("right.tasks.stop"), 11.0, theme::text())),
            );
        }
        detail.child(actions).into_any_element()
    }

    /// Start a new attempt for a settled turn task in one workspace.
    fn retry_turn_in(&mut self, root: &std::path::Path, id: &str) {
        use crate::extension::runs::{Limits, RunSpec, RunTier, Runs};
        use crate::extension::tasks::{
            Attempt, AttemptStatus, FileTasksStore, Host, TaskStatus, TasksStore, Tier,
        };
        let Ok(Some(mut record)) = FileTasksStore.load(root, id) else {
            return;
        };
        if !matches!(record.host, Host::Local) || record.tier != Tier::Turn {
            return;
        }
        if record.active_attempt().is_some() || !record.may_retry() {
            return;
        }
        let Some(service) = self.task_runs.clone() else {
            return;
        };
        let spec = RunSpec {
            session: record.origin.session.clone(),
            objective: record.intent.clone(),
            tier: RunTier::Turn,
            limits: Limits {
                steps: record.budget.max_steps,
                tokens: record.budget.max_tokens,
                deadline_ms: record.budget.deadline_ms,
            },
            target: None,
            capability: None,
            trace: record.id.clone(),
        };
        let Ok(handle) = Runs::with(service).start(spec) else {
            return;
        };
        let now = crate::settings::unix_now().saturating_mul(1_000);
        let attempt_id = format!("{}-{}", record.id, record.attempts.len() + 1);
        let mut attempt = Attempt::queued(attempt_id, Tier::Turn);
        attempt.run = Some(handle.id.clone());
        attempt.status = AttemptStatus::Running;
        attempt.started_ms = Some(now);
        record.attempts.push(attempt);
        record.status = TaskStatus::Running;
        record.updated_ms = now;
        let _ = FileTasksStore.save(root, &record);
        self.right.tasks.borrow_mut().take();
    }

    /// Retry a settled local turn task from the pane.
    pub(crate) fn retry_task_turn(&mut self, id: &str) {
        let root = self.task_workspace_root();
        self.retry_turn_in(&root, id);
    }

    /// Start a new attempt for failed turn tasks whose policy is automatic.
    pub(crate) fn auto_retry_failed_turns(&mut self) {
        use crate::extension::tasks::{
            FileTasksStore, Host, RetryMode, TaskStatus, TasksStore, Tier,
        };
        let roots = self.task_roots_for_pane();
        for root in roots {
            let Ok(tasks) = FileTasksStore.list(&root) else {
                continue;
            };
            for record in tasks {
                if !matches!(record.host, Host::Local) || record.tier != Tier::Turn {
                    continue;
                }
                if record.status != TaskStatus::Failed {
                    continue;
                }
                if record.retry.mode != RetryMode::Auto || !record.may_retry() {
                    continue;
                }
                self.retry_turn_in(&root, &record.id);
            }
        }
    }

    /// Delete the settled records of the active session's workspace.
    pub(crate) fn clear_completed_tasks(&mut self) {
        use crate::extension::tasks::{FileTasksStore, TaskStatus, TasksStore};
        let root = self.task_workspace_root();
        let Ok(tasks) = FileTasksStore.list(&root) else {
            return;
        };
        for task in tasks {
            let settled = matches!(
                task.status,
                TaskStatus::Succeeded
                    | TaskStatus::Failed
                    | TaskStatus::Cancelled
                    | TaskStatus::Interrupted
            );
            if settled {
                let _ = FileTasksStore.delete(&root, &task.id);
            }
        }
        self.right.tasks.borrow_mut().take();
        self.right.expanded.clear();
    }

    /// Stop a local task from the pane. Peer work is stopped from the tool,
    /// whose async seam can reach the peer.
    pub(crate) fn stop_task_local(&mut self, id: &str) {
        use crate::extension::runs::RunsService as _;
        use crate::extension::tasks::{
            AttemptStatus, FileTasksStore, Host, TaskStatus, TasksStore, Tier,
        };
        let root = self.task_workspace_root();
        let Ok(Some(mut task)) = FileTasksStore.load(&root, id) else {
            return;
        };
        if !matches!(task.host, Host::Local) {
            return;
        }
        let run = task.attempts.last().and_then(|attempt| attempt.run.clone());
        if let Some(run) = &run {
            match task.tier {
                Tier::Turn => {
                    if let Some(service) = &self.task_runs {
                        let _ = service.cancel(run);
                    }
                }
                Tier::Subagent => {
                    let _ = self.state.subagents.interrupt(run);
                }
                Tier::Job => {
                    self.state.jobs.kill(run);
                }
                Tier::Delegation | Tier::PeerTask => return,
            }
        }
        let now = crate::settings::unix_now().saturating_mul(1_000);
        task.status = TaskStatus::Cancelled;
        task.updated_ms = now;
        if let Some(attempt) = task.attempts.last_mut() {
            attempt.status = AttemptStatus::Cancelled;
            attempt.ended_ms = Some(now);
        }
        let _ = FileTasksStore.save(&root, &task);
        self.right.tasks.borrow_mut().take();
    }

    fn task_section_key(kind: crate::extension::tasks::BoardSectionKind) -> &'static str {
        use crate::extension::tasks::BoardSectionKind;
        match kind {
            BoardSectionKind::Running => "right.tasks.running",
            BoardSectionKind::Queued => "right.tasks.queued",
            BoardSectionKind::Scheduled => "right.tasks.scheduled",
            BoardSectionKind::History => "right.tasks.history",
        }
    }

    /// The status dot's colour.
    fn task_status_color(status: crate::extension::tasks::TaskStatus) -> u32 {
        use crate::extension::tasks::TaskStatus;
        match status {
            TaskStatus::Running => theme::accent(),
            TaskStatus::Blocked | TaskStatus::Unknown => theme::warn(),
            TaskStatus::Failed => theme::error(),
            TaskStatus::Succeeded => theme::success(),
            TaskStatus::Scheduled | TaskStatus::Queued | TaskStatus::Waiting => theme::faint(),
            TaskStatus::Cancelled | TaskStatus::Interrupted => theme::muted(),
        }
    }

    /// The federation pane: the node's health, peers, work, resources and
    /// knowledge, drawn by the pane's own module.
    fn federation_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        super::federation_pane::pane(self, cx)
    }

    /// Read the peer list from the running node when there is one, otherwise
    /// from the file, and keep the selection valid.
    pub(crate) fn reload_federation(&mut self) {
        self.reload_federation_with(true);
    }

    /// Re-read the node and the store without announcing; the discovery tick
    /// uses this, so a quiet peer does not get a frame's worth of chatter.
    pub(crate) fn reload_federation_quiet(&mut self) {
        self.reload_federation_with(false);
    }

    fn reload_federation_with(&mut self, announce: bool) {
        let runtime = crate::federation_node::running();
        if announce {
            if let Some(runtime) = &runtime {
                // A refresh also says "still here", which is what makes a peer
                // that started late appear on the other side without waiting
                // for the next interval.
                runtime.reannounce();
            }
        }
        let rows = if let Some(runtime) = &runtime {
            runtime
                .node()
                .peers()
                .read()
                .map(|store| federation_rows(&store))
                .unwrap_or_default()
        } else if let Some(path) = self.state.settings.federation_peers_path() {
            federation::PeerStore::load(&path)
                .map(|store| federation_rows(&store))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if let Some(selected) = self.right.federation.selected {
            if selected >= rows.len() {
                self.right.federation.selected = None;
                self.right.federation.grant_choices.clear();
            }
        }
        self.right.federation.peers = rows;
        self.right.federation.discovered = runtime
            .as_ref()
            .map(|runtime| {
                runtime
                    .announced_peers()
                    .into_iter()
                    .map(|record| DiscoveredPeerRow {
                        node: record.node.to_string(),
                        name: record.name.clone(),
                        capabilities: record
                            .descriptors
                            .iter()
                            .map(|descriptor| {
                                (
                                    descriptor.id.to_string(),
                                    descriptor.risk.as_str().to_string(),
                                )
                            })
                            .collect(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.right.federation.delegations = runtime
            .as_ref()
            .map(|runtime| {
                let store = runtime.node().delegations();
                let store = store
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                federation_delegation_rows(&store)
            })
            .unwrap_or_default();
        self.right.federation.artifacts =
            federation_artifact_rows(&self.state.settings.federation_dir().join("artifacts"));
        self.right.federation.knowledge_items = runtime
            .as_ref()
            .map(|runtime| {
                runtime
                    .node()
                    .knowledge_items()
                    .into_iter()
                    .map(|item| FederationKnowledgeRow {
                        id: item.id,
                        class: format!("{:?}", item.class).to_lowercase(),
                        source: item.provenance.source,
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.right.federation.leases = runtime
            .as_ref()
            .map(|runtime| {
                let leases = runtime.node().leases();
                let leases = leases
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                federation_lease_rows(&leases)
            })
            .unwrap_or_default();
        if announce {
            // A manifest scan, not a host start: the list is cheap enough for a
            // tab open and a Refresh, and too expensive for the two-second
            // discovery tick, which passes announce = false.
            self.right.federation.extension_tools =
                crate::extension::host::declared_tool_catalogue(&self.state.settings);
        }
        self.right.federation.loaded = true;
    }

    /// Apply one change to the peer store and persist it.
    pub(crate) fn apply_federation<F>(&mut self, apply: F, cx: &mut Context<Self>)
    where
        F: FnOnce(&mut federation::PeerStore) -> Result<String, String>,
    {
        let result = match crate::federation_node::running() {
            Some(runtime) => match runtime.node().peers().write() {
                Ok(mut store) => apply(&mut store).and_then(|notice| {
                    if let Some(path) = self.state.settings.federation_peers_path() {
                        store.save(&path).map_err(|error| error.to_string())?;
                    }
                    Ok(notice)
                }),
                Err(_) => Err("the peer store is poisoned".to_string()),
            },
            None => match self.state.settings.federation_peers_path() {
                Some(path) => mutate_store_file(&path, apply),
                None => Err("federation is not configured in this build".to_string()),
            },
        };
        match result {
            Ok(notice) => {
                self.right.federation.notice = Some(notice);
                self.right.federation.error = None;
            }
            Err(error) => {
                self.right.federation.error = Some(error);
                self.right.federation.notice = None;
            }
        }
        self.reload_federation();
        cx.notify();
    }
    /// A pane that has not been built yet says so, rather than drawing an
    /// empty box the reader has to interpret.
    fn right_pane_placeholder(&self, tab: RightTab) -> impl IntoElement {
        let lang = self.state.lang;
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(8.0))
            .px(px(theme::RIGHT_PAD * 2.0))
            .child(icon(tab.glyph(), 22.0, theme::faint()))
            .child(
                txt(lang.tr(tab.key()), theme::SIDEBAR_SIZE, theme::text())
                    .font_weight(FontWeight::MEDIUM),
            )
            .child(
                txt(lang.tr("right.notBuilt"), theme::CHIP_SIZE, theme::faint())
                    .text_align(TextAlign::Center),
            )
    }

    /// The built-in Browser pane.
    ///
    /// It is a text view, not a rendered page, and it draws only what the
    /// document model actually holds: the address bar, the four navigation
    /// controls, the loading or failure line, and the extracted title, URL,
    /// text and interactive elements. Nothing here paints a page the browser
    /// did not read.
    fn browser_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = self.browser_pane.clone().unwrap_or_default();
        let mut pane = div()
            .debug_selector(|| String::from("browser-pane"))
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(rgb(theme::page()));
        pane = pane.child(
            div()
                .flex_none()
                .w_full()
                .h(px(36.0))
                .px(px(theme::RIGHT_PAD))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .child(self.browser_icon(
                    "browser-back",
                    IconName::ArrowLeft,
                    view.can_back || self.browser_webview.is_some(),
                    "right.browser.back",
                    cx,
                    |this, cx| this.browser_history(-1, cx),
                ))
                .child(self.browser_icon(
                    "browser-forward",
                    IconName::ArrowRight,
                    view.can_forward || self.browser_webview.is_some(),
                    "right.browser.forward",
                    cx,
                    |this, cx| this.browser_history(1, cx),
                ))
                .child(self.browser_icon(
                    "browser-reload",
                    IconName::RotateCw,
                    view.snapshot.is_some() || self.browser_webview.is_some(),
                    "right.browser.reload",
                    cx,
                    |this, cx| this.browser_reload(cx),
                ))
                .child(
                    div()
                        .debug_selector(|| String::from("browser-address"))
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&self.browser_url).appearance(false).h(px(26.0))),
                )
                .child(self.browser_go_control(cx)),
        );
        // The native view IS the live pane; the CDP engine path stays for the
        // text/agent backend.
        let live = self.state.browser.engine_ready() || self.browser_webview.is_some();
        if live {
            pane.child(self.browser_live_body(cx))
        } else {
            // The text view is the fallback, and a render failure is said
            // ABOVE it in the engine's own words: never a blank rectangle and
            // never a silent downgrade.
            pane.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(self.browser_engine_notice())
                    .child(self.browser_body(view, cx)),
            )
        }
    }

    /// One navigation control: a glyph that dims when the action cannot run.
    fn browser_icon(
        &self,
        id: &'static str,
        glyph: IconName,
        enabled: bool,
        label: &'static str,
        cx: &mut Context<Self>,
        action: fn(&mut Shell, &mut Context<Shell>),
    ) -> impl IntoElement {
        let tooltip = self.state.lang.tr(label);
        let mut button = div()
            .id(id)
            .debug_selector(move || String::from(id))
            .w(px(26.0))
            .h(px(26.0))
            .flex_none()
            .rounded(px(theme::RADIUS_SM))
            .flex()
            .items_center()
            .justify_center()
            .tooltip_show_delay(std::time::Duration::from_millis(500))
            .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx));
        if enabled {
            button = button
                .cursor_pointer()
                .hover(|style| style.bg(rgb(theme::hover())))
                .child(icon(glyph, 14.0, theme::muted()))
                .on_click(cx.listener(move |this, _event, _window, cx| action(this, cx)));
        } else {
            button = button.child(icon(glyph, 14.0, theme::faint()));
        }
        button
    }

    /// The Go control.
    fn browser_go_control(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("browser-go")
            .debug_selector(|| String::from("browser-go"))
            .h(px(26.0))
            .px(px(10.0))
            .rounded(px(theme::RADIUS_SM))
            .border_1()
            .border_color(rgb(theme::border()))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(theme::hover())))
            .flex()
            .items_center()
            .child(
                txt(
                    self.state.lang.tr("right.browser.go"),
                    theme::CHIP_SIZE,
                    theme::text(),
                )
                .font_weight(FontWeight::MEDIUM),
            )
            .on_click(cx.listener(|this, _event, _window, cx| this.browser_submit(cx)))
    }

    /// The pane's body: the empty state, the loading line, a failure, or the
    /// page itself.
    fn browser_body(&self, view: crate::browser::PaneView, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let state_id = format!("browser-state-{}", view.status.as_str());
        let mut body = div()
            .debug_selector(move || state_id.clone())
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .overflow_hidden();
        if let Some(failure) = view.failure.clone() {
            body = body.child(self.browser_failure(failure));
        }
        match view.status {
            crate::browser::Status::Empty => body
                .child(
                    div()
                        .flex_1()
                        .w_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(8.0))
                        .px(px(theme::RIGHT_PAD * 2.0))
                        .child(icon(IconName::Globe, 22.0, theme::faint()))
                        .child(
                            txt(
                                lang.tr("right.browser.empty"),
                                theme::CHIP_SIZE,
                                theme::faint(),
                            )
                            .text_align(TextAlign::Center)
                            .line_height(px(18.0)),
                        ),
                )
                .into_any_element(),
            crate::browser::Status::Loading => {
                let line = crate::i18n::tr_args(
                    lang,
                    "right.browser.loading",
                    &[("url", view.url.clone())],
                );
                body.child(
                    div()
                        .debug_selector(|| String::from("browser-loading"))
                        .w_full()
                        .px(px(theme::RIGHT_PAD))
                        .py(px(8.0))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(icon(IconName::RotateCw, 12.0, theme::muted()))
                        .child(txt(line, theme::CHIP_SIZE, theme::muted()).truncate()),
                )
                .into_any_element()
            }
            crate::browser::Status::Ready | crate::browser::Status::Failed => match view.snapshot {
                Some(snapshot) => body
                    .child(self.browser_page(snapshot, cx))
                    .into_any_element(),
                None => body
                    .child(
                        div()
                            .flex_1()
                            .w_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(txt(
                                lang.tr("right.browser.empty"),
                                theme::CHIP_SIZE,
                                theme::faint(),
                            )),
                    )
                    .into_any_element(),
            },
        }
    }

    /// The failure line: what happened, and the rule when the policy was the
    /// reason. It sits above the page, because a failed navigation keeps the
    /// page the reader was already reading.
    fn browser_failure(&self, failure: crate::browser::Failure) -> impl IntoElement {
        let lang = self.state.lang;
        let mut note = div()
            .debug_selector(|| String::from("browser-failure"))
            .flex_none()
            .w_full()
            .px(px(theme::RIGHT_PAD))
            .py(px(8.0))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .bg(rgb(theme::hover()))
            .child(
                txt(lang.tr(failure.key()), theme::CHIP_SIZE, theme::error())
                    .font_weight(FontWeight::MEDIUM),
            )
            .child(
                // The producer's own prose: with the browser direct there is no
                // setting of the reader's that could be the thing refusing.
                txt(failure.message.clone(), theme::CHIP_SIZE, theme::text()).line_height(px(16.0)),
            );
        if let Some(rule) = failure.rule.clone() {
            note = note.child(txt(
                crate::i18n::tr_args(lang, "right.browser.rule", &[("rule", rule)]),
                theme::RIGHT_HINT_SIZE,
                theme::faint(),
            ));
        }
        note
    }

    /// The extracted page: title, URL, capped text and the interactive rows.
    fn browser_page(
        &self,
        snapshot: crate::browser::Snapshot,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let lang = self.state.lang;
        let mut page = div()
            .id("browser-page")
            .debug_selector(|| String::from("browser-page"))
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .px(px(theme::RIGHT_PAD))
            .py(px(8.0))
            .overflow_y_scroll();
        page = page
            .child(
                div()
                    .debug_selector(|| String::from("browser-title"))
                    .w_full()
                    .child(
                        txt(snapshot.title.clone(), theme::SIDEBAR_SIZE, theme::text())
                            .font_weight(FontWeight::MEDIUM)
                            .line_height(px(19.0)),
                    )
                    .child(
                        txt(snapshot.url.clone(), theme::RIGHT_HINT_SIZE, theme::faint())
                            .truncate()
                            .line_height(px(15.0)),
                    ),
            )
            .child(
                div()
                    .debug_selector(|| String::from("browser-text"))
                    .w_full()
                    .child(
                        txt(snapshot.text.clone(), theme::CHIP_SIZE, theme::text())
                            .line_height(px(18.0)),
                    ),
            );
        if snapshot.text_truncated {
            let note = crate::i18n::tr_args(
                lang,
                "right.browser.capped",
                &[
                    ("cap", snapshot.text_cap.to_string()),
                    ("total", snapshot.text_total_chars.to_string()),
                ],
            );
            page =
                page.child(txt(note, theme::RIGHT_HINT_SIZE, theme::faint()).line_height(px(15.0)));
        }
        // The element rows: the refs the model may pass back, and the only
        // place the pane shows that a page has controls at all.
        let heading = crate::i18n::tr_args(
            lang,
            "right.browser.elements",
            &[("count", snapshot.elements_total.to_string())],
        );
        page = page.child(div().w_full().pt(px(4.0)).child(
            txt(heading, theme::RIGHT_HINT_SIZE, theme::muted()).font_weight(FontWeight::MEDIUM),
        ));
        if snapshot.elements.is_empty() {
            page = page.child(
                txt(
                    lang.tr("right.browser.noElements"),
                    theme::RIGHT_HINT_SIZE,
                    theme::faint(),
                )
                .line_height(px(15.0)),
            );
        }
        for element in snapshot.elements.iter() {
            page = page.child(self.browser_element_row(element.clone(), cx));
        }
        if snapshot.elements_total > snapshot.elements.len() {
            let note = crate::i18n::tr_args(
                lang,
                "right.browser.cappedElements",
                &[
                    ("shown", snapshot.elements.len().to_string()),
                    ("total", snapshot.elements_total.to_string()),
                ],
            );
            page =
                page.child(txt(note, theme::RIGHT_HINT_SIZE, theme::faint()).line_height(px(15.0)));
        }
        page
    }

    /// One interactive element as a row.
    ///
    /// A link row is a real control: pressing it navigates through the same
    /// gate the address bar uses. Every other row is a readout, because this
    /// view cannot type into a field from the reader's side without a per-field
    /// editor - the model does that through browser_type, and drawing a
    /// pointer cursor on a row that does nothing would be the exact "control
    /// that only looks wired" this crate refuses.
    fn browser_element_row(
        &self,
        element: crate::browser::ElementView,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let reference = element.reference.clone();
        let id = element.reference.clone();
        let mut row = div()
            .id(SharedString::from(format!("browser-element-{id}")))
            .debug_selector(move || format!("browser-element-{id}"))
            .w_full()
            .min_h(px(22.0))
            .px(px(4.0))
            .rounded(px(4.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .child(
                txt(reference, theme::RIGHT_HINT_SIZE, theme::accent())
                    .font_family("monospace")
                    .flex_none(),
            )
            .child(txt(element.kind, theme::RIGHT_HINT_SIZE, theme::faint()).flex_none());
        let label = if let Some(value) = element.value.clone() {
            if value.is_empty() {
                element.label.clone()
            } else {
                format!("{} = {}", element.label, value)
            }
        } else {
            element.label.clone()
        };
        row = row.child(txt(label, theme::CHIP_SIZE, theme::text()).truncate());
        if let Some(href) = element.href.clone() {
            if element.kind == "link" {
                row = row
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(theme::hover())))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.browser_go(href.clone(), cx);
                    }));
            }
        }
        row
    }

    /// The terminal pane: libghostty's own surface, drawn by Ghostty.
    ///
    /// The pane keeps the key path because the policy gates a submitted line
    /// before the shell sees it; everything else - the grid, the cursor, the
    /// scrollback, selection and mouse reporting - belongs to Ghostty.
    fn terminal_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.right.terminal.is_none() {
            return self
                .right_pane_placeholder(RightTab::Terminal)
                .into_any_element();
        }
        // The IME reaches a view only through the window's input handler, which
        // is set during paint; without it a Chinese keyboard composes nothing
        // the shell can read.
        let ime_focus = self.terminal_focus.clone();
        let ime_panel = cx.entity();
        let mut pane = div()
            .debug_selector(|| String::from("terminal-pane"))
            .id("terminal-pane")
            .flex_1()
            .min_h_0()
            .w_full()
            .h_full()
            .relative()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(rgb(theme::page()))
            .cursor_text()
            .track_focus(&self.terminal_focus)
            // Tab is bound in this context (see terminal_tab): without a
            // binding, the macOS input handler hands it to AppKit's key-view
            // loop and focus leaves the pane instead of the shell completing.
            .key_context("terminal")
            .on_action(cx.listener(|this, _: &TerminalTab, _window, cx| {
                this.terminal_tab(cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.terminal_key(event, window, cx);
            }))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, _window, cx| {
                this.terminal_key_up(event, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.terminal_press(event, window, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.terminal_press(event, window, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                this.terminal_move(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                    this.terminal_release(event, cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                    this.terminal_release(event, cx);
                }),
            )
            .on_scroll(cx.listener(|this, event: &ScrollEvent, _window, cx| {
                this.terminal_scroll(event, cx);
            }));
        // The strip sits ABOVE the surface instead of over it: the native view
        // is a layer of the window, so anything the pane paints underneath it
        // would be covered.
        let running = self
            .right
            .terminal_view
            .as_ref()
            .is_some_and(|view| view.read(cx).is_alive());
        if let Some((text, is_error)) = self.terminal_status_text(running) {
            pane = pane.child(
                div()
                    .debug_selector(|| String::from("terminal-status"))
                    .id("terminal-status")
                    .flex_none()
                    .px(px(12.0))
                    .py(px(8.0))
                    .bg(rgb(theme::page()))
                    .child(txt(
                        text,
                        theme::CHIP_SIZE,
                        if is_error {
                            theme::error()
                        } else {
                            theme::faint()
                        },
                    )),
            );
        }
        match self.right.terminal_view.clone() {
            Some(view) => {
                pane = pane.child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h(px(0.0))
                        .w_full()
                        .child(view)
                        .child(
                            // The input handler is registered during PAINT:
                            // Window::handle_input refuses to run in prepaint.
                            canvas(
                                |_, _, _| {},
                                move |bounds, _, window, cx| {
                                    window.handle_input(
                                        &ime_focus,
                                        gpui::ElementInputHandler::new(bounds, ime_panel.clone()),
                                        cx,
                                    );
                                },
                            )
                            .absolute()
                            .size_full(),
                        ),
                );
            }
            // No surface: the start gate refused it, Ghostty could not attach
            // to this window, or the first spawn failed. The strip above says
            // which, and Enter is the way back.
            None => {
                pane = pane.child(div().flex_1().min_h(px(0.0)).w_full());
            }
        }
        pane.into_any_element()
    }

    /// One keystroke to the shell.
    ///
    /// Every key but a submitted line goes straight to libghostty, which owns
    /// the terminal's input encoding (arrows, control chords, the kitty
    /// protocol, mouse reporting). Enter is the one gated key: the line this
    /// pane has mirrored since the last submit is shown to the policy, and a
    /// refusal sends Ctrl-U so the shell's line editor erases what it echoed,
    /// instead of the carriage return.
    pub(crate) fn terminal_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.platform {
            // Copy and paste belong to the terminal: Ghostty owns the
            // selection and the clipboard, so it has to see those two chords.
            // Everything else with the platform modifier stays the
            // application's - the shell never sees Cmd-Q, and Ghostty's own
            // Cmd-W would close the surface under the pane.
            let key = keystroke.key.as_str();
            if key == "c" || key == "v" {
                if let Some(view) = self.right.terminal_view.clone() {
                    view.update(cx, |view, _| view.key_down(keystroke, event.is_held));
                }
            }
            return;
        }
        let Some(view) = self.right.terminal_view.clone() else {
            crate::terminal::trace_key(&keystroke.key, None);
            return;
        };
        if terminal_is_enter(keystroke) {
            crate::terminal::trace_key(&keystroke.key, Some(b"\r"));
            cx.stop_propagation();
            if !view.read(cx).is_alive() {
                // A dead pane's Enter is the way back: a fresh shell, under the
                // same start gate.
                self.restart_terminal(window, cx);
                return;
            }
            let outcome = self.right.terminal.as_mut().map(|terminal| {
                terminal.submit(&mut |emission| {
                    view.update(cx, |view, _| view.emission(emission));
                    true
                })
            });
            // A refusal is shown in the status line, so the pane has to redraw.
            if matches!(outcome, Some(KeyOutcome::Refused)) {
                cx.notify();
            }
            return;
        }
        if let Some(edit) = terminal_edit(keystroke) {
            if let Some(terminal) = self.right.terminal.as_mut() {
                terminal.track(edit);
            }
        }
        crate::terminal::trace_key(&keystroke.key, None);
        view.update(cx, |view, _| {
            view.set_focused(true);
            view.key_down(keystroke, event.is_held);
        });
        cx.stop_propagation();
    }

    /// Forward a released key: the kitty protocol reports releases.
    pub(crate) fn terminal_key_up(&mut self, event: &KeyUpEvent, cx: &mut Context<Self>) {
        let Some(view) = self.right.terminal_view.clone() else {
            return;
        };
        view.update(cx, |view, _| view.key_up(&event.keystroke));
    }

    /// Tab belongs to the shell.
    ///
    /// The macOS input handler passes an unbound Tab to AppKit's key-view
    /// loop, which moves focus out of the pane before the shell sees the byte,
    /// so Tab is bound to this action in the pane's terminal key context and
    /// forwarded to Ghostty as the key it is.
    pub(crate) fn terminal_tab(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.right.terminal_view.clone() else {
            return;
        };
        view.update(cx, |view, _| view.tab());
        cx.stop_propagation();
    }

    /// A press puts the keyboard in the pane and tells Ghostty where it landed.
    ///
    /// Selection, word selection and copy-on-select are Ghostty's own: it sees
    /// the press and every move that follows.
    pub(crate) fn terminal_press(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.terminal_focus, cx);
        if let Some(view) = self.right.terminal_view.clone() {
            view.update(cx, |view, _| {
                view.set_focused(true);
                view.mouse_move(event.position, event.modifiers);
                view.mouse_down(event.position, event.button, event.modifiers);
            });
        }
    }

    /// A pointer move: hover, a drag's selection, and a motion report.
    pub(crate) fn terminal_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if let Some(view) = self.right.terminal_view.clone() {
            view.update(cx, |view, _| {
                view.mouse_move(event.position, event.modifiers);
            });
        }
    }

    /// A release ends whatever the press began.
    pub(crate) fn terminal_release(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        if let Some(view) = self.right.terminal_view.clone() {
            view.update(cx, |view, _| {
                view.mouse_move(event.position, event.modifiers);
                view.mouse_up(event.position, event.button, event.modifiers);
            });
        }
    }

    /// The wheel: Ghostty scrolls its own retained scrollback.
    pub(crate) fn terminal_scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        if let Some(view) = self.right.terminal_view.clone() {
            view.update(cx, |view, _| {
                view.scroll(event.position, event.delta, event.modifiers);
            });
            cx.stop_propagation();
        }
    }

    /// Tell the surface whether the pane owns the keyboard.
    fn terminal_focus_changed(&mut self, focused: bool, cx: &mut Context<Self>) {
        if let Some(view) = self.right.terminal_view.clone() {
            view.update(cx, |view, _| view.set_focused(focused));
        }
    }

    /// Enter on a dead terminal: a fresh surface for the same shell.
    ///
    /// The start gate is asked again by the model, so a policy that refuses the
    /// shell refuses this too. The dead surface is dropped rather than reused:
    /// Ghostty keeps the exited shell's last frame, and the pane's restart has
    /// always been a turn of the page.
    fn restart_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(terminal) = self.right.terminal.as_mut() else {
            return;
        };
        if !terminal.restart() {
            cx.notify();
            return;
        }
        self.right.terminal_view = None;
        self.right.terminal_view_subs.clear();
        self.ensure_terminal_view(window, cx);
        self.sync_terminal(cx);
        window.focus(&self.terminal_focus, cx);
        cx.notify();
    }

    /// Create the pane's libghostty surface, once.
    ///
    /// The start gate ran when the terminal was built: a refusal or a policy
    /// error leaves the pane with no surface at all and the status line says
    /// why. A spawn failure is recorded on the model so the status line reports
    /// it and Enter can try again.
    fn ensure_terminal_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.right.terminal_view.clone() {
            // Coming back from another tab: the surface was hidden, not
            // dropped, so it keeps the running shell.
            view.update(cx, |view, _| view.set_visible(true));
            return;
        }
        let Some(terminal) = self.right.terminal.as_ref() else {
            return;
        };
        if terminal.refused {
            return;
        }
        let options = TerminalOptions::new(terminal.command(), terminal.cwd.clone());
        let font_size =
            crate::theme::terminal_font_size(self.state.draft().global.general.content_font_size);
        match TerminalView::spawn(options, font_size, window, cx) {
            Ok(view) => {
                let focus_in = cx.on_focus(&self.terminal_focus, window, |this, _window, cx| {
                    this.terminal_focus_changed(true, cx);
                });
                let focus_out = cx.on_blur(&self.terminal_focus, window, |this, _window, cx| {
                    this.terminal_focus_changed(false, cx);
                });
                // A window that is not key draws a hollow cursor: the surface
                // is told once per activation change rather than per frame.
                let activation = cx.observe_window_activation(window, |this, window, cx| {
                    let focused =
                        this.terminal_focus.is_focused(window) && window.is_window_active();
                    this.terminal_focus_changed(focused, cx);
                });
                self.right.terminal_view = Some(view);
                self.right.terminal_view_subs = vec![focus_in, focus_out, activation];
                if let Some(terminal) = self.right.terminal.as_mut() {
                    terminal.failure = None;
                }
                self.sync_terminal(cx);
            }
            Err(error) => {
                if let Some(terminal) = self.right.terminal.as_mut() {
                    terminal.failure = Some(error);
                }
                cx.notify();
            }
        }
    }

    /// Hide the surface when another pane takes the body.
    ///
    /// It is a layer of the window's view, so leaving it up would draw the
    /// terminal over whatever tab replaced it.
    pub(crate) fn hide_terminal_view(&mut self, cx: &mut Context<Self>) {
        if let Some(view) = self.right.terminal_view.clone() {
            view.update(cx, |view, _| view.set_visible(false));
        }
    }

    /// What the pane says when there is no shell to draw.
    ///
    /// A refused start reports the policy's own copy, and a shell that failed
    /// or exited reports the runtime reason beside it. Both are errors: a
    /// terminal that cannot take a command is not a blank terminal.
    pub(crate) fn terminal_status_text(&self, running: bool) -> Option<(String, bool)> {
        let terminal = self.right.terminal.as_ref()?;
        let lang = self.state.lang;
        if terminal.refused {
            let mut text = lang.tr("right.terminal.refused");
            if let Some(reason) = terminal.failure.as_ref() {
                text.push_str(": ");
                text.push_str(reason);
            }
            // A refused start spawned nothing, so there is no shell whose
            // confinement could be described: the refusal is the whole line.
            return Some((text, true));
        }
        // A RUNNING shell carries no chrome: the pane's status line is for what
        // went wrong, and a standing warning band under a working shell would be
        // prose explaining state. The sandbox answer reaches the reader two
        // other ways instead: the shell's own BSH_SANDBOX, and this line while
        // the pane has something to report (below).
        let mut parts: Vec<String> = Vec::new();
        // A policy that turned the sandbox off is a standing security fact, not
        // a transient error: say it while the shell runs, not only after exit,
        // and name WHICH policy - the settings master switch or the preset in
        // force, because the reader reverses them with different controls.
        match terminal.sandbox_off {
            Some(crate::terminal::SandboxOff::Settings) => {
                parts.push(lang.tr("right.terminal.sandboxOffSettings"));
            }
            Some(crate::terminal::SandboxOff::Preset) => {
                parts.push(lang.tr("right.terminal.sandboxOff"));
            }
            None => {}
        }
        // A terminal SPAWNS once and keeps that spec. A later flip of the
        // master switch - OR a change of the session permission - does not
        // rewrite a running process, so when the pane's spawn-time confinement
        // class differs from the one a terminal opened NOW would get, say so
        // instead of letting the silence read as "it changed with the switch".
        //
        // The class is three-valued and cheap: no fresh spec is built on a
        // render, because deriving one probes the policy for every grant.
        let spawned = terminal_confinement(terminal.sandbox_off, terminal.sandbox_label());
        let eff = self.state.effective();
        let current = if eff.sandbox.off_by_settings || eff.sandbox.off_by_preset {
            TerminalConfinement::PolicyReleased
        } else if crate::settings::resolve::is_full_access(&eff.permission.effective)
            && !eff.sandbox.heel
        {
            // Full access with no preset asking for heel: the permission
            // releases the shell, and a new terminal would report "full-access".
            TerminalConfinement::PermissionReleased
        } else {
            TerminalConfinement::Confined
        };
        if spawned != current {
            parts.push(lang.tr("right.terminal.keepsSpawnSpec"));
        }
        if let Some(error) = terminal.policy_error() {
            parts.push(format!(
                "{}: {error}",
                lang.tr("right.terminal.policyError")
            ));
        }
        if let Some(refusal) = terminal.last_refusal() {
            let mut text = format!(
                "{}: {}",
                lang.tr("right.terminal.inputRefused"),
                refusal.line
            );
            if let Some(reason) = refusal.reason.as_ref() {
                text.push_str(": ");
                text.push_str(reason);
            }
            parts.push(text);
        }
        // The audit's compact face: a count, and only once something was
        // actually refused or redacted. No other chrome is added for a policy.
        let blocked = terminal.blocked_decisions();
        if blocked > 0 {
            parts.push(crate::i18n::tr_args(
                lang,
                "right.terminal.policyBlocked",
                &[("count", blocked.to_string())],
            ));
        }
        if !running {
            let mut text = lang.tr("right.terminal.notRunning");
            if let Some(reason) = terminal.failure.as_ref() {
                text.push_str(": ");
                text.push_str(reason);
            }
            // A dead pane must say how to come back: Enter starts a new shell
            // (see Terminal::restart). Without this the pane looked hung after
            // `exit`, because nothing on screen named the way out.
            if !terminal.refused {
                text.push_str(" \u{b7} ");
                text.push_str(&lang.tr("right.terminal.restartHint"));
            }
            // A pane that is already speaking says what the spec answered too:
            // the refusal to confine, or the tier the shell actually ran under.
            // The policy-off sentence was already added above; it is not a
            // backend limitation and must not be dressed as one.
            if terminal.sandbox_off.is_none() {
                if let Some(note) = terminal.sandbox_note() {
                    text.push_str(" \u{b7} ");
                    text.push_str(&format!(
                        "{}: {note}",
                        lang.tr("right.terminal.unsandboxed")
                    ));
                } else if let Some(label) = terminal.sandbox_label() {
                    text.push_str(" \u{b7} ");
                    text.push_str(&crate::i18n::tr_args(
                        lang,
                        "right.terminal.sandboxTier",
                        &[("tier", label.to_string())],
                    ));
                }
            }
            parts.insert(0, text);
        }
        if parts.is_empty() {
            return None;
        }
        Some((parts.join(" \u{b7} "), true))
    }

    /// The Start screen as Finder's Applications page: the category tabs, then
    /// a grid of item cards for the panes and the open documents.
    ///
    /// The grid is the point. The old Start screen was a centred column of
    /// 380px cards, so at any useful panel height a handful of panes filled it
    /// and the rest required scrolling; square cards give three or four columns
    /// at the same width, and the category row plus the top search field narrow
    /// the grid without leaving the page.
    fn right_start(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        let (card_w, gap) = self.start_card_size();
        let query = self.right_search_text.trim().to_lowercase();
        let scope = self.right_search_scope;
        let mut page = div()
            .id("right-start")
            .debug_selector(|| String::from("right-start"))
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col();
        page = page.child(self.right_search_bar());
        page = page.child(self.start_filter_row(cx));

        let mut body = div()
            .id("right-start-body")
            .debug_selector(|| String::from("right-start-body"))
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .overflow_y_scroll();

        let mut grid = div()
            .id("right-start-grid")
            .debug_selector(|| String::from("right-start-grid"))
            .w_full()
            .p(px(16.0))
            .flex()
            .flex_row()
            .flex_wrap()
            .justify_start()
            .content_start()
            .gap(px(gap));
        let mut shown = 0usize;
        if scope != RightSearchScope::Documents {
            for tab in RightTab::ALL
                .into_iter()
                .filter(|tab| tab.on_start_screen())
            {
                let label = lang.tr(tab.key());
                if !query.is_empty() && !label.to_lowercase().contains(&query) {
                    continue;
                }
                shown += 1;
                grid = grid.child(self.right_start_card(tab, card_w, cx));
            }
        }
        if scope != RightSearchScope::Panes {
            for (index, tab) in self.files.tabs.iter().enumerate() {
                let label = tab.label();
                let rel = tab.rel.display().to_string();
                if !query.is_empty()
                    && !label.to_lowercase().contains(&query)
                    && !rel.to_lowercase().contains(&query)
                {
                    continue;
                }
                shown += 1;
                grid = grid.child(self.right_start_file_card(index, tab, card_w, cx));
            }
        }
        if shown == 0 {
            grid = grid.child(
                div()
                    .w_full()
                    .py(px(36.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.0))
                    .child(icon(IconName::Search, 20.0, theme::faint()))
                    .child(txt(lang.tr("right.search.noResults"), 13.0, theme::muted())),
            );
        }
        body = body.child(grid);
        body = body.child(self.start_recent_section(cx));
        page = page.child(body);
        page = page.child(self.start_footer(cx));
        page
    }

    /// The category tabs above the Start grid. They set the same scope the
    /// search field's filter control does, so the two never disagree.
    fn start_filter_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        let mut row = div()
            .h(px(36.0))
            .flex_none()
            .px(px(16.0))
            .flex()
            .flex_row()
            .items_center();
        for choice in [
            RightSearchScope::All,
            RightSearchScope::Panes,
            RightSearchScope::Documents,
        ] {
            let active = self.right_search_scope == choice;
            let slug = choice.slug();
            let glyph = match choice {
                RightSearchScope::All => IconName::LayoutGrid,
                RightSearchScope::Panes => IconName::LayoutDashboard,
                RightSearchScope::Documents => IconName::FileText,
            };
            row = row.child(
                div()
                    .id(SharedString::from(format!("right-start-filter-{slug}")))
                    .debug_selector(move || format!("right-start-filter-{slug}"))
                    .relative()
                    .flex_1()
                    .h(px(36.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(6.0))
                    .cursor_pointer()
                    .child(icon(
                        glyph,
                        14.0,
                        if active {
                            theme::accent()
                        } else {
                            theme::muted()
                        },
                    ))
                    .child(
                        txt(
                            lang.tr(choice.key()),
                            13.0,
                            if active {
                                theme::accent()
                            } else {
                                theme::muted()
                            },
                        )
                        .font_weight(FontWeight::MEDIUM),
                    )
                    .when(active, |tab| {
                        tab.child(
                            div()
                                .absolute()
                                .bottom(px(0.0))
                                .left(px(0.0))
                                .right(px(0.0))
                                .h(px(2.0))
                                .rounded(px(1.0))
                                .bg(rgb(theme::accent())),
                        )
                    })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.right_search_scope = choice;
                        cx.notify();
                    })),
            );
        }
        hairline_bottom(row).border_color(rgb(theme::border_soft()))
    }

    /// The recent rows the current scope and query show, newest first.
    fn launcher_recent_rows(&self) -> Vec<crate::recent::RecentEntry> {
        let query = self.right_search_text.trim().to_lowercase();
        let scope = self.right_search_scope;
        self.state
            .launcher_recent
            .entries()
            .iter()
            .filter(|entry| match scope {
                RightSearchScope::All => true,
                RightSearchScope::Panes => entry.kind == crate::recent::RecentKind::Panel,
                RightSearchScope::Documents => entry.kind == crate::recent::RecentKind::Document,
            })
            .filter(|entry| {
                query.is_empty()
                    || entry.label.to_lowercase().contains(&query)
                    || entry.detail.to_lowercase().contains(&query)
            })
            .cloned()
            .collect()
    }

    /// The launcher's automatically maintained recent rows.
    ///
    /// The section is filtered by the same scope and query as the grid, so a
    /// reader searching "bos" sees the workspace as well as matching panes. The
    /// rows are runtime records, not favorites; the only control is removing
    /// one, which is offered behind the row's own menu.
    fn start_recent_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let now = agent::capability::unix_ms();
        let entries = self.launcher_recent_rows();
        let total = entries.len();
        let open = self.right.launcher_recent_open;
        let limit = if open { 20 } else { 3 };

        let mut section = div()
            .w_full()
            .px(px(16.0))
            .pt(px(8.0))
            .pb(px(12.0))
            .border_t_1()
            .border_color(rgb(theme::border_soft()))
            .flex()
            .flex_col()
            .gap(px(3.0));
        let mut header = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .pb(px(4.0))
            .child(icon(IconName::Clock, 14.0, theme::muted()))
            .child(
                txt(lang.tr("right.start.recent"), 12.0, theme::text())
                    .font_weight(FontWeight::MEDIUM),
            )
            .child(div().flex_1());
        if total > 3 {
            header = header.child(
                div()
                    .id("right-recent-toggle")
                    .debug_selector(|| "right-recent-toggle".to_string())
                    .px(px(4.0))
                    .h(px(22.0))
                    .rounded(px(6.0))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(theme::hover())))
                    .child(txt(
                        lang.tr(if open {
                            "right.start.showLess"
                        } else {
                            "right.start.viewMore"
                        }),
                        10.0,
                        theme::accent(),
                    ))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.right.launcher_recent_open = !this.right.launcher_recent_open;
                        cx.notify();
                    })),
            );
        }
        section = section.child(header);

        if total == 0 {
            if !self.right_search_text.trim().is_empty() {
                return section.into_any_element();
            }
            section = section.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(8.0))
                    .py(px(10.0))
                    .rounded(px(9.0))
                    .bg(rgb(theme::new_session_fill()))
                    .child(icon(IconName::Clock, 14.0, theme::faint()))
                    .child(txt(
                        lang.tr("right.start.recentEmpty"),
                        10.5,
                        theme::faint(),
                    )),
            );
            return section.into_any_element();
        }

        for (index, entry) in entries.iter().take(limit).enumerate() {
            let entry_for_click = entry.clone();
            let entry_for_menu = entry.clone();
            let (glyph, tint) = match entry.kind {
                crate::recent::RecentKind::Workspace => (IconName::FolderOpen, theme::accent()),
                crate::recent::RecentKind::Document => {
                    (file_glyph_for_name(&entry.label), 0x7C5CFC)
                }
                crate::recent::RecentKind::Panel => RightTab::from_slug(&entry.id)
                    .map(Self::start_card_meta)
                    .unwrap_or((IconName::PanelsTopLeft, theme::muted())),
            };
            // Panel labels are translated at render, not stored: a row written
            // under one language must not stay in it after a language change.
            let (row_label, row_detail) = match entry.kind {
                crate::recent::RecentKind::Panel => RightTab::from_slug(&entry.id)
                    .map(|tab| (lang.tr(tab.key()), lang.tr(tab.description_key())))
                    .unwrap_or_else(|| (entry.label.clone(), entry.detail.clone())),
                _ => (entry.label.clone(), entry.detail.clone()),
            };
            let when = Self::recent_time_label(lang, now, entry.opened_at_ms);
            let shell = cx.entity();
            let remove_kind = entry.kind;
            let remove_id = entry.id.clone();
            let menu_button = Button::new(SharedString::from(format!("right-recent-more-{index}")))
                .ghost()
                .w(px(24.0))
                .h(px(24.0))
                .rounded(px(6.0))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .child(txt("⋮", 14.0, theme::faint()))
                .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                    let open_shell = shell.clone();
                    let open_entry = entry_for_menu.clone();
                    let remove_shell = shell.clone();
                    let remove_kind = remove_kind;
                    let remove_id = remove_id.clone();
                    menu.item(
                        PopupMenuItem::new(crate::i18n::tr(lang, "right.start.open")).on_click(
                            move |_, window, cx| {
                                let entry = open_entry.clone();
                                open_shell.update(cx, |this, cx| {
                                    this.open_launcher_recent(entry, window, cx);
                                });
                            },
                        ),
                    )
                    .item(
                        PopupMenuItem::new(crate::i18n::tr(lang, "right.start.removeRecent"))
                            .on_click(move |_, _, cx| {
                                remove_shell.update(cx, |this, cx| {
                                    this.state.remove_launcher_recent(remove_kind, &remove_id);
                                    cx.notify();
                                });
                            }),
                    )
                });

            section = section.child(
                div()
                    .id(SharedString::from(format!("right-recent-{index}")))
                    .debug_selector(move || format!("right-recent-{index}"))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .px(px(8.0))
                    .py(px(7.0))
                    .rounded(px(10.0))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(theme::new_session_fill())))
                    .child(
                        div()
                            .flex_none()
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(8.0))
                            .bg(rgb(theme::soft(tint)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(glyph, 14.0, tint)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(1.0))
                            .flex_1()
                            .min_w_0()
                            .child(txt(row_label.clone(), 11.5, theme::text()).text_ellipsis())
                            .child(txt(row_detail.clone(), 9.5, theme::faint()).text_ellipsis()),
                    )
                    .child(txt(when, 9.5, theme::faint()))
                    .child(
                        div()
                            .id(SharedString::from(format!("right-recent-menu-{index}")))
                            .flex_none()
                            .on_click(cx.listener(|_, _, _, cx| cx.stop_propagation()))
                            .child(menu_button),
                    )
                    .on_click(cx.listener(move |this, _event, window, cx| {
                        this.open_launcher_recent(entry_for_click.clone(), window, cx);
                    })),
            );
        }
        section.into_any_element()
    }

    /// The one-line age of a recent row.
    fn recent_time_label(lang: crate::i18n::Lang, now_ms: u64, opened_ms: u64) -> String {
        let elapsed = now_ms.saturating_sub(opened_ms);
        let minutes = elapsed / 60_000;
        if minutes == 0 {
            return lang.tr("right.start.justNow");
        }
        if minutes < 60 {
            return crate::i18n::tr_args(
                lang,
                "right.start.minutesAgo",
                &[("count", minutes.to_string())],
            );
        }
        let hours = minutes / 60;
        if hours < 24 {
            return crate::i18n::tr_args(
                lang,
                "right.start.hoursAgo",
                &[("count", hours.to_string())],
            );
        }
        crate::i18n::tr_args(
            lang,
            "right.start.daysAgo",
            &[("count", (hours / 24).to_string())],
        )
    }

    /// Open one recent row: a panel, a document in its workspace, or a root.
    fn open_launcher_recent(
        &mut self,
        entry: crate::recent::RecentEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clear_right_search(window, cx);
        self.right.open = true;
        match entry.kind {
            crate::recent::RecentKind::Panel => {
                let Some(tab) = RightTab::from_slug(&entry.id) else {
                    return;
                };
                self.open_right_tab(tab, window, cx);
                if tab == RightTab::Terminal {
                    window.focus(&self.terminal_focus, cx);
                }
            }
            crate::recent::RecentKind::Document => {
                if let Some(workspace) = &entry.workspace {
                    if workspace != &self.state.settings.global.general.active_workspace {
                        self.state.set_workspace(workspace);
                        self.refile_workspace(cx);
                    }
                }
                self.open_file(std::path::Path::new(&entry.id), window, cx);
            }
            crate::recent::RecentKind::Workspace => {
                self.record_workspace_recent(&entry.id);
                self.state.set_workspace(&entry.id);
                self.refile_workspace(cx);
                cx.notify();
            }
        }
    }

    /// The launcher's footer: the startup preference and the primary Open.
    fn start_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let checked = self.state.settings.global.general.launcher_on_start;
        let can_open =
            !self.right_search_matches().is_empty() || !self.launcher_recent_rows().is_empty();
        let checkbox = div()
            .id("right-launcher-startup")
            .debug_selector(|| "right-launcher-startup".to_string())
            .w(px(16.0))
            .h(px(16.0))
            .flex_none()
            .rounded(px(4.0))
            .border_1()
            .border_color(rgb(if checked {
                theme::accent()
            } else {
                theme::border_strong()
            }))
            .bg(rgb(if checked {
                theme::accent()
            } else {
                theme::page()
            }))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .when(checked, |check| {
                check.child(icon(IconName::Check, 11.0, 0xFFFFFF))
            })
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.state.settings.global.general.launcher_on_start = !checked;
                this.save_settings(cx);
                cx.notify();
            }));
        let open = div()
            .id("right-launcher-open")
            .debug_selector(|| "right-launcher-open".to_string())
            .h(px(34.0))
            .px(px(16.0))
            .rounded(px(9.0))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.0))
            .bg(rgb(theme::accent()))
            .opacity(if can_open { 1.0 } else { 0.4 })
            .when(can_open, |button| button.cursor_pointer())
            .child(icon(IconName::Play, 14.0, 0xFFFFFF))
            .child(txt(lang.tr("right.start.open"), 12.0, 0xFFFFFF).font_weight(FontWeight::MEDIUM))
            .on_click(cx.listener(move |this, _event, window, cx| {
                if can_open {
                    this.open_first_launcher_item(window, cx);
                }
            }));
        div()
            .h(px(52.0))
            .flex_none()
            .px(px(16.0))
            .border_t_1()
            .border_color(rgb(theme::border_soft()))
            .bg(rgb(theme::page()))
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(checkbox)
            .child(txt(
                lang.tr("right.start.showAtStartup"),
                11.0,
                theme::muted(),
            ))
            .child(div().flex_1())
            .child(open)
            .into_any_element()
    }

    /// The grid card size at this panel width. Three columns at the default
    /// panel width, never wider than 200px.
    fn start_card_size(&self) -> (f32, f32) {
        let gap = 12.0;
        let pad = 16.0;
        let width = (self.right_panel_w() - 1.0).max(0.0);
        let columns = (((width - pad * 2.0 + gap) / 124.0).floor() as usize).clamp(2, 6);
        let card = ((width - pad * 2.0 - gap * (columns - 1) as f32) / columns as f32).min(160.0);
        (card, gap)
    }

    /// The launcher's semantic colour and glyph for one pane.
    ///
    /// Colour is not decoration: it is the fast path to the right tile in a grid
    /// the reader scans, so each family keeps one hue (files blue, terminal green,
    /// browser violet, extensions amber, federation teal, tasks blue).
    fn start_card_meta(tab: RightTab) -> (IconName, u32) {
        const VIOLET: u32 = 0x7C5CFC;
        const AMBER: u32 = 0xE8912D;
        const TEAL: u32 = 0x0E9F9F;
        match tab {
            RightTab::Files => (IconName::FolderOpen, theme::accent()),
            RightTab::Terminal => (IconName::SquareTerminal, theme::success()),
            RightTab::Diff => (IconName::GitCompareArrows, theme::muted()),
            RightTab::Browser => (IconName::Globe, VIOLET),
            RightTab::Extensions => (IconName::Puzzle, AMBER),
            RightTab::Federation => (IconName::GitBranch, TEAL),
            RightTab::Tasks => (IconName::ClipboardList, 0x3B82F6),
        }
    }

    /// One Start card, as an Applications-style icon tile.
    ///
    /// The tile is the whole target: a 26px glyph over the pane's name, with
    /// the shortcut underneath when one is bound. That is what lets three
    /// columns fit in the default panel where the old 380px rows fit one.
    fn right_start_card(
        &self,
        tab: RightTab,
        width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let lang = self.state.lang;
        let label = lang.tr(tab.key());
        let description = lang.tr(tab.description_key());
        let (glyph, tint) = Self::start_card_meta(tab);
        // A pane with no key binding draws no hint: the shortcut is a fact
        // about the shell, and printing one nothing dispatches would be the
        // exact "control that only looks wired" this crate refuses.
        let hint = match tab {
            RightTab::Files => Some("\u{2318}P"),
            RightTab::Diff => None,
            RightTab::Terminal => Some("\u{2325}\u{60}"),
            RightTab::Browser => Some("\u{2318}T"),
            RightTab::Extensions => None,
            RightTab::Federation => None,
            RightTab::Tasks => None,
        };
        let tip = match hint {
            Some(hint) => format!("{label}  {hint}"),
            None => label.clone(),
        };
        div()
            .id(SharedString::from(format!("right-card-{}", tab.key())))
            .debug_selector(move || format!("right-card-{}", tab.key()))
            .w(px(width))
            .h(px(118.0))
            .flex_none()
            .p(px(8.0))
            .rounded(px(12.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(theme::hover())))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(5.0))
            .tooltip_show_delay(std::time::Duration::from_millis(500))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .child(
                div()
                    .flex_none()
                    .w(px(48.0))
                    .h(px(48.0))
                    .rounded(px(13.0))
                    .bg(rgb(theme::soft(tint)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(glyph, 24.0, tint)),
            )
            .child(
                txt(label, 12.5, theme::text())
                    .font_weight(FontWeight::MEDIUM)
                    .min_w_0()
                    .text_ellipsis(),
            )
            .child(
                txt(description, 10.0, theme::faint())
                    .min_w_0()
                    .whitespace_nowrap()
                    .text_ellipsis(),
            )
            .on_click(cx.listener(move |this, _event, window, cx| {
                this.clear_right_search(window, cx);
                this.open_right_tab(tab, window, cx);
                // A terminal is a text surface: opening it puts the cursor
                // where the reader's next keystroke will land.
                if tab == RightTab::Terminal {
                    window.focus(&this.terminal_focus, cx);
                }
            }))
    }

    /// One open document as a Start-grid tile.
    fn right_start_file_card(
        &self,
        index: usize,
        tab: &FileTab,
        width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let label = tab.label();
        let path = tab.rel.display().to_string();
        let tip_path = path.clone();
        const VIOLET: u32 = 0x7C5CFC;
        div()
            .id(SharedString::from(format!("right-start-file-{index}")))
            .debug_selector(move || format!("right-start-file-{index}"))
            .w(px(width))
            .h(px(118.0))
            .flex_none()
            .p(px(8.0))
            .rounded(px(12.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(theme::hover())))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(5.0))
            .tooltip_show_delay(std::time::Duration::from_millis(500))
            .tooltip(move |window, cx| Tooltip::new(tip_path.clone()).build(window, cx))
            .child(
                div()
                    .flex_none()
                    .w(px(48.0))
                    .h(px(48.0))
                    .rounded(px(13.0))
                    .bg(rgb(theme::soft(VIOLET)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(file_glyph_for_name(&label), 24.0, VIOLET)),
            )
            .child(
                txt(label, 12.5, theme::text())
                    .font_weight(FontWeight::MEDIUM)
                    .min_w_0()
                    .text_ellipsis(),
            )
            .child(
                txt(path, 10.0, theme::faint())
                    .min_w_0()
                    .whitespace_nowrap()
                    .text_ellipsis(),
            )
            .on_click(cx.listener(move |this, _event, window, cx| {
                this.clear_right_search(window, cx);
                this.focus_tab(index, cx);
            }))
    }

    /// The Workspace Files pane: one full-width surface, the tree or the
    /// document, with only the chrome that surface needs.
    ///
    /// Measured from the reference's own dock: an open document takes the whole
    /// panel and the tree comes back through the Files chip. A document's
    /// surface is the editor and nothing else - the tab chip already names the
    /// file and cmd-s saves it, so a path bar and a save button over it would
    /// only repeat what the reader already has.
    fn files_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = div()
            .debug_selector(|| String::from("files-pane"))
            // cmd-alt-m: the keyboard path to a comment, so a reader marking
            // code does not have to find the + first. It bubbles here from the
            // focused editor.
            .on_action(cx.listener(|this, _: &AddComment, _window, cx| {
                this.open_comment_card(cx);
            }))
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .overflow_hidden();
        match self.state.workspace_root() {
            None => pane.child(self.files_no_workspace()),
            Some(root) => {
                pane.when(self.files.active_tab().is_none(), |pane| {
                    pane.child(self.files_header(&root, cx))
                })
                // The new-file row appears only when a double-click on the
                // tree's blank space asked for it (see `blank_clicked`).
                .when(
                    self.files.active_tab().is_none() && self.files.new_file_open,
                    |pane| pane.child(self.files_new_row(cx)),
                )
                // A rename is its own row rather than a tenant of the create
                // row: arming one from a tree row has to show the field whether
                // or not the create field happens to be open. It used to be
                // nested inside it, so the pencil set a flag nothing drew and a
                // rename could not be typed at all.
                .when(
                    self.files.active_tab().is_none() && self.files.renaming.is_some(),
                    |pane| pane.child(self.files_rename_row(cx)),
                )
                // The save-failure line stays on both surfaces: feedback is
                // not chrome, and it only renders when something went wrong.
                .when_some(self.files.error.clone(), |pane, error| {
                    pane.child(
                        div()
                            .debug_selector(|| String::from("files-error"))
                            .px(px(theme::RIGHT_PAD))
                            .pb(px(4.0))
                            .flex_none()
                            .child(
                                txt(error, theme::CHIP_SIZE, theme::error()).line_height(px(16.0)),
                            ),
                    )
                })
                .when_some(self.files.active_tab(), |pane, _open| {
                    pane.child(self.files_editor(&root, cx))
                })
                .when(self.files.active_tab().is_none(), |pane| {
                    pane.child(self.files_listing(cx))
                })
            }
        }
    }

    /// The pane with no workspace selected.
    fn files_no_workspace(&self) -> impl IntoElement {
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(8.0))
            .px(px(theme::RIGHT_PAD * 2.0))
            .child(icon(IconName::FolderClosed, 22.0, theme::faint()))
            .child(
                txt(
                    self.state.lang.tr("right.files.noWorkspace"),
                    theme::CHIP_SIZE,
                    theme::faint(),
                )
                .text_align(TextAlign::Center)
                .line_height(px(18.0)),
            )
    }

    /// The 38px header bar, plus the rename row while one is in flight.
    ///
    /// The new-file field that used to sit here as a permanent strip is gone:
    /// the reader asks for it with a double-click on the tree's blank space
    /// (see [`Shell::blank_double_clicked`]), so the tree's own chrome is just
    /// the path and its tools.
    fn files_header(&self, root: &Path, cx: &mut Context<Self>) -> impl IntoElement {
        let path = root.display().to_string();
        // The header itself is the reference's measured 38px bar: path on the
        // left, tools on the right, a .5px rule under it. The new-file and
        // rename fields no longer ride here — the gesture that summons them
        // lives on the tree's blank space (see [`Shell::blank_double_clicked`]),
        // so this bar stays the measured 38px and nothing more.
        div()
            .debug_selector(|| String::from("files-header"))
            .flex_none()
            .w_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .debug_selector(|| String::from("files-path"))
                    .w_full()
                    .h(px(theme::FILES_HEADER_H))
                    .flex_none()
                    // Measured: padding 0 6px 0 16px, gap 4.
                    .pl(px(16.0))
                    .pr(px(6.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .border_b_1()
                    .border_color(rgb(theme::border_soft()))
                    .child(icon(IconName::FolderOpen, 13.0, theme::faint()).flex_none())
                    .child(
                        txt(path, theme::CHIP_SIZE, theme::muted())
                            .line_height(px(18.0))
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap(),
                    )
                    .child(
                        div()
                            .id("files-refresh")
                            .debug_selector(|| String::from("files-refresh"))
                            // Measured tool: 28x28, radius-sm, 6px padding, 15px svg.
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(theme::RADIUS_SM))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(theme::hover())))
                            .child(icon(IconName::RefreshCw, 15.0, theme::muted()))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.refresh_files(cx);
                            })),
                    )
                    .child(
                        div()
                            .id("files-hidden")
                            .debug_selector(|| String::from("files-hidden"))
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(theme::RADIUS_SM))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .when(self.files.hidden, |s| s.bg(rgb(theme::hover())))
                            .hover(|s| s.bg(rgb(theme::hover())))
                            .child(icon(
                                IconName::Eye,
                                15.0,
                                if self.files.hidden {
                                    theme::text()
                                } else {
                                    theme::muted()
                                },
                            ))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.files.hidden = !this.files.hidden;
                                this.refresh_files(cx);
                            })),
                    ),
            )
    }

    /// The new-file field, shown only after a double-click on the tree's blank
    /// space.
    ///
    /// The permanent manage strip is gone: creating a file is a gesture, not
    /// chrome that sits on screen. This row is that gesture made visible, and
    /// it carries the same rename field below it when a rename is in flight so
    /// both mutations share one place to type.
    fn files_new_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        div()
            .debug_selector(|| String::from("files-manage"))
            .w_full()
            .flex_none()
            .px(px(theme::RIGHT_PAD))
            .pt(px(6.0))
            .pb(px(2.0))
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .debug_selector(|| String::from("files-new"))
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .debug_selector(|| String::from("files-new-input"))
                            .flex_1()
                            .min_w_0()
                            .child(
                                Input::new(&self.files.new_name)
                                    .appearance(false)
                                    .h(px(28.0)),
                            ),
                    )
                    .child(
                        div()
                            .id("files-create")
                            .debug_selector(|| String::from("files-create"))
                            .h(px(28.0))
                            .px(px(10.0))
                            .rounded(px(theme::RADIUS_SM))
                            .border_1()
                            .border_color(rgb(theme::border()))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(theme::hover())))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(4.0))
                            .child(icon(IconName::Plus, 13.0, theme::text()))
                            .child(
                                txt(lang.tr("right.files.new"), theme::CHIP_SIZE, theme::text())
                                    .font_weight(FontWeight::MEDIUM),
                            )
                            .on_click(cx.listener(|this, _event, window, cx| {
                                this.create_file(window, cx);
                            })),
                    ),
            )
    }

    /// The rename row, which only exists while a row is being renamed.
    fn files_rename_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        let target = self
            .files
            .renaming
            .clone()
            .unwrap_or_default()
            .display()
            .to_string();
        div()
            .debug_selector(|| String::from("files-rename"))
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .child(
                txt(target, theme::CHIP_SIZE, theme::faint())
                    .flex_none()
                    .max_w(px(120.0))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap(),
            )
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&self.files.rename_name)
                        .appearance(false)
                        .h(px(28.0)),
                ),
            )
            .child(
                div()
                    .id("files-rename-confirm")
                    .debug_selector(|| String::from("files-rename-confirm"))
                    .h(px(28.0))
                    .px(px(10.0))
                    .rounded(px(theme::RADIUS_SM))
                    .border_1()
                    .border_color(rgb(theme::accent()))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(theme::hover())))
                    .child(
                        txt(
                            lang.tr("right.files.renameConfirm"),
                            theme::CHIP_SIZE,
                            theme::text(),
                        )
                        .font_weight(FontWeight::MEDIUM),
                    )
                    .on_click(cx.listener(|this, _event, window, cx| {
                        this.rename_file(window, cx);
                    })),
            )
    }

    /// The directory tree: the workspace root's entries, each expanded branch
    /// nested under its parent at 18px a level.
    ///
    /// Measured from the reference's own FilesBody: nested `.level` lists with
    /// `.level .level { padding-left: 18px }`. The walk is depth-first over
    /// cached listings only, so a keystroke never touches the filesystem.
    fn files_listing(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div()
            .id("files-list")
            .debug_selector(|| String::from("files-list"))
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .w_full()
            .flex()
            .flex_col();
        // Measured body padding: 8px top, 8px left, 0 right.
        list = list.pt(px(8.0)).pl(px(8.0));
        if self.files.roots.is_empty() {
            list = list.child(div().pr(px(theme::RIGHT_PAD)).pb(px(8.0)).child(txt(
                self.state.lang.tr("right.files.empty"),
                theme::CHIP_SIZE,
                theme::faint(),
            )));
        }
        let mut stack: Vec<(&files::Row, PathBuf, usize)> = self
            .files
            .roots
            .iter()
            .map(|row| (row, PathBuf::new(), 0))
            .collect();
        // Reversed so a pop walks the entries in listing order.
        stack.reverse();
        while let Some((row, parent, depth)) = stack.pop() {
            let rel = parent.join(&row.name);
            list = list.child(self.files_tree_row(row, &rel, depth, cx));
            if row.dir {
                let expanded = self.files.expanded.iter().any(|p| p == &rel);
                if expanded {
                    if let Some(children) = self.files.children.get(&rel) {
                        for child in children.iter().rev() {
                            stack.push((child, rel.clone(), depth + 1));
                        }
                    }
                }
            }
        }
        // A double-click on the space below the last row asks for a new file
        // there. gpui exposes no on_double_click, and its ClickEvent's
        // click_count is unreliable for a synthesized press, so the pane reads
        // the gesture itself: two presses close together in time and space.
        // The blank div fills the pane's remaining height, so the gesture has
        // somewhere to land that is never a row.
        list.child(
            div()
                .id("files-blank")
                .debug_selector(|| String::from("files-blank"))
                .w_full()
                .flex_1()
                .min_h(px(48.0))
                .cursor_pointer()
                .on_click(cx.listener(|this, event: &gpui::ClickEvent, window, cx| {
                    let gpui::ClickEvent::Mouse(mouse) = event else {
                        return;
                    };
                    let now = std::time::Instant::now();
                    let here = mouse.up.position;
                    let is_double = match this.files.last_blank_press {
                        Some((there, then)) => {
                            let near = (here - there).magnitude() < f64::from(SELF_DOUBLE_PX);
                            let soon = now.duration_since(then)
                                < std::time::Duration::from_millis(SELF_DOUBLE_MS);
                            near && soon
                        }
                        None => false,
                    };
                    // A double-click consumes the press it was built from, so
                    // the next click starts a fresh sequence rather than reading
                    // as yet another double-click. The same reset applies when
                    // a lone press closes the field, so the reader's next
                    // double-click isn't mistaken for a single.
                    let consumed = is_double || this.files.new_file_open;
                    this.files.last_blank_press = if consumed { None } else { Some((here, now)) };
                    if is_double {
                        this.blank_double_clicked(window, cx);
                    } else if this.files.new_file_open {
                        // A lone press on blank space retires the new-file
                        // field, so the tree stays quiet when the reader is
                        // just clearing the gesture away.
                        this.files.new_file_open = false;
                        cx.notify();
                    }
                })),
        )
    }

    /// One row of the tree, with the actions that appear on hover.
    ///
    /// Measured from the reference's own `.row`: `padding: 5px 10px`, a 6px
    /// gap, and a `radius-md` rounded hover fill. The height is left to the
    /// 13px/1.5 line box rather than pinned, so a bigger system font grows the
    /// row instead of clipping inside it. A directory carries a chevron that
    /// turns with its branch, and `depth` sets the 18px-per-level indent the
    /// reference nests its lists with.
    fn files_tree_row(
        &self,
        row: &files::Row,
        rel: &Path,
        depth: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let name = row.name.clone();
        let dir = row.dir;
        let rel_click = rel.to_path_buf();
        let rel_diff = rel.to_path_buf();
        let rel_rename = rel.to_path_buf();
        let rel_delete = rel.to_path_buf();
        let rel_toggle = rel.to_path_buf();
        // Element ids and the hover group key on the whole path: two branches
        // can hold a file of the same name, and a duplicated id shares one
        // element state between them - the hover fill and both action groups
        // would light up together. The debug selector stays name-based, which
        // is what the click tests address.
        let rel_key = rel.to_string_lossy().to_string();
        let group = SharedString::from(format!("file-{rel_key}"));
        let row_id = SharedString::from(format!("file-row-{rel_key}"));
        // The debug selector stays name-based: it is a test address, not
        // element state, and the click tests name their row by file name.
        let debug_id = SharedString::from(format!("file-row-{name}"));
        let rename_id = SharedString::from(format!("rename-{rel_key}"));
        let delete_id = SharedString::from(format!("delete-{rel_key}"));
        let row_group = group.clone();
        let size = row.size;
        let deleting = self.files.deleting.as_deref() == Some(rel);
        let renaming = self.files.renaming.as_deref() == Some(rel);
        let expanded = self.files.expanded.iter().any(|p| p == rel);
        let glyph = file_glyph(row);
        let shell = cx.entity();
        div()
            .debug_selector(|| debug_id.to_string())
            .id(row_id)
            .group(group)
            .w_full()
            .min_h(px(0.0))
            .rounded(px(theme::RADIUS_SM))
            .pl(px(10.0 + 18.0 * depth as f32))
            .pr(px(10.0))
            .py(px(5.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .cursor_pointer()
            .when(renaming, |row| row.bg(rgb(theme::selected_row())))
            .when(!renaming, |row| row.hover(|s| s.bg(rgb(theme::hover()))))
            .when(dir, |row| {
                row.child(
                    icon(
                        if expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        },
                        12.0,
                        theme::faint(),
                    )
                    .flex_none(),
                )
            })
            .when(!dir, |row| row.child(div().w(px(12.0)).flex_none()))
            .child(icon(glyph, 14.0, theme::faint()).flex_none())
            .child(
                txt(name.clone(), theme::SIDEBAR_SIZE, theme::text())
                    .truncate()
                    .flex_1(),
            )
            // The size is what a reader scans for, so it holds the row's right
            // edge until an action fades over it.
            .child(
                txt(size_label(size, dir), theme::CHIP_SIZE, theme::faint())
                    .flex_none()
                    .group_hover(row_group.clone(), |s| s.opacity(0.0)),
            )
            .child(
                div()
                    .absolute()
                    .right(px(theme::RIGHT_PAD))
                    .top(px(0.0))
                    .h(px(theme::RIGHT_ROW_H))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(2.0))
                    // A directory has nothing to compare, so the action is a file's
                    // only. It is the pane's entry point: the Diff tab alone can
                    // never load anything, because a comparison needs a FILE.
                    .when(!dir, |strip| {
                        strip.child(
                            div()
                                .debug_selector(|| format!("diff-{}", row.name))
                                .id(SharedString::from(format!("diff-{}", row.name.clone())))
                                .px(px(4.0))
                                .rounded(px(4.0))
                                .cursor_pointer()
                                .opacity(0.0)
                                .group_hover(row_group.clone(), |s| s.opacity(1.0))
                                .hover(|s| s.bg(rgb(theme::hover())))
                                .child(icon(IconName::GitCompareArrows, 12.0, theme::faint()))
                                .on_click({
                                    let shell = shell.clone();
                                    let rel = rel_diff.clone();
                                    move |_event, _window, cx| {
                                        cx.stop_propagation();
                                        shell.update(cx, |this, cx| {
                                            this.open_diff(&rel, cx);
                                        });
                                    }
                                }),
                        )
                    })
                    .child(
                        div()
                            .id(rename_id)
                            .debug_selector(|| format!("rename-{}", row.name))
                            .px(px(4.0))
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .opacity(0.0)
                            .group_hover(row_group.clone(), |s| s.opacity(1.0))
                            .hover(|s| s.bg(rgb(theme::hover())))
                            .child(icon(IconName::Pencil, 12.0, theme::faint()))
                            .on_click({
                                let shell = shell.clone();
                                let rel = rel_rename.clone();
                                move |_event, window, cx| {
                                    cx.stop_propagation();
                                    shell.update(cx, |this, cx| {
                                        this.begin_rename(&rel, window, cx);
                                    });
                                }
                            }),
                    )
                    .child(
                        div()
                            .id(delete_id)
                            .debug_selector(|| format!("delete-{}", row.name))
                            .px(px(4.0))
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .opacity(0.0)
                            .group_hover(row_group.clone(), |s| s.opacity(1.0))
                            .hover(|s| s.bg(rgb(theme::hover())))
                            .child(icon(
                                IconName::Trash,
                                12.0,
                                if deleting {
                                    theme::error()
                                } else {
                                    theme::faint()
                                },
                            ))
                            .on_click({
                                let shell = shell.clone();
                                let rel = rel_delete.clone();
                                move |_event, _window, cx| {
                                    cx.stop_propagation();
                                    shell.update(cx, |this, cx| {
                                        this.press_delete(&rel, cx);
                                    });
                                }
                            }),
                    ),
            )
            .on_click(cx.listener(move |this, _event, window, cx| {
                if dir {
                    this.toggle_dir(&rel_toggle, cx);
                } else {
                    this.open_file(&rel_click, window, cx);
                }
            }))
    }

    /// One open file's chip, drawn in the panel's own tab strip.
    ///
    /// A chip carries the file's own name, the dirty dot when its buffer has
    /// changes, and the close that drops it. Pressing a chip focuses that tab
    /// rather than reopening the file, which is what makes the strip a strip
    /// and not a list of buttons.
    fn files_tab_chip(
        &self,
        index: usize,
        tab: &FileTab,
        width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // The document is only "on screen" while Files holds the body: another
        // pane can be up with the same buffer still open, and a chip painted as
        // selected then would name a body the reader cannot see.
        let selected = self.right.tab == Some(RightTab::Files) && self.files.active == Some(index);
        let name = tab.label();
        let chip_name = name.clone();
        let dirty = tab.dirty;
        let text_color = if selected {
            theme::text()
        } else {
            theme::muted()
        };
        let weight = if selected {
            FontWeight::MEDIUM
        } else {
            FontWeight::default()
        };
        let close_name = name.clone();
        let documents = self.files.tabs.len();
        let other_tabs = documents + self.right.open_panes.len().saturating_sub(1);
        let full_path = tab.rel.display().to_string();
        let lang = self.state.lang;
        let entity = cx.entity();
        div()
            .debug_selector(move || format!("file-tab-{chip_name}"))
            .id(SharedString::from(format!("file-tab-{index}")))
            .w(px(width))
            .h(px(28.0))
            .flex_none()
            .min_w_0()
            .px(px(8.0))
            .rounded(px(theme::RADIUS_SM))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .overflow_hidden()
            .cursor_pointer()
            .when(selected, |chip| chip.bg(rgb(theme::selected_row())))
            .hover(|s| s.bg(rgb(theme::hover())))
            // A name that the tab had to clip still has a full path worth
            // reading, which is what Finder's own tab tooltip shows.
            .tooltip_show_delay(std::time::Duration::from_millis(500))
            .tooltip(move |window, cx| Tooltip::new(full_path.clone()).build(window, cx))
            .child(match file_type_badge(&name) {
                Some((badge, color)) => div()
                    .w(px(18.0))
                    .h(px(18.0))
                    .flex_none()
                    .rounded(px(4.0))
                    .bg(rgb(color))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(txt(badge, 8.0, 0xFFFFFF).font_weight(FontWeight::BOLD))
                    .into_any_element(),
                None => icon(file_glyph_for_name(&name), 12.0, theme::faint())
                    .flex_none()
                    .into_any_element(),
            })
            .child(
                txt(name, theme::CHIP_SIZE, text_color)
                    .font_weight(weight)
                    .flex_1()
                    .min_w_0()
                    .text_ellipsis(),
            )
            // A note glyph rides the chip the way the dirty dot does, so a
            // reader sees at a glance that the file carries remarks.
            .when(self.files.notes.has_notes(&tab.rel), |chip| {
                chip.child(icon(IconName::MessageSquare, 11.0, theme::muted()).flex_none())
            })
            // The dirty dot sits after the name and before the close, so it
            // reads as a property of the file rather than of the chip's edge.
            .when(dirty, |chip| {
                chip.child(
                    div()
                        .w(px(6.0))
                        .h(px(6.0))
                        .rounded_full()
                        .bg(rgb(theme::warn()))
                        .flex_none(),
                )
            })
            // Only the selected tab draws its close, so the inactive chips
            // spend that width on the file name instead.
            .when(selected, |chip| {
                chip.child(
                    div()
                        .id(SharedString::from(format!("file-tab-close-{index}")))
                        .debug_selector(move || format!("file-tab-close-{close_name}"))
                        .px(px(3.0))
                        .flex_none()
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(theme::hover())))
                        .child(icon(IconName::X, 11.0, theme::faint()))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            cx.stop_propagation();
                            this.close_tab(index, cx);
                        })),
                )
            })
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.focus_tab(index, cx);
            }))
            .context_menu(move |menu, _, _| {
                let close = entity.clone();
                let others = entity.clone();
                let all = entity.clone();
                let show = entity.clone();
                menu.item(
                    PopupMenuItem::new(lang.tr("right.close")).on_click(move |_, _, cx| {
                        close.update(cx, |this, cx| this.close_tab(index, cx));
                    }),
                )
                .item(
                    PopupMenuItem::new(lang.tr("right.closeOthers"))
                        .disabled(other_tabs < 2)
                        .on_click(move |_, _, cx| {
                            others.update(cx, |this, cx| this.close_other_tabs(index, cx));
                        }),
                )
                .item(
                    PopupMenuItem::new(lang.tr("right.closeAll")).on_click(move |_, _, cx| {
                        all.update(cx, |this, cx| this.close_all_tabs(cx));
                    }),
                )
                .item(PopupMenuItem::separator())
                // Finder's catalogue command, on the tab where a reader looks
                // for the rest of the tab operations.
                .item(PopupMenuItem::new(lang.tr("right.showAllTabs")).on_click(
                    move |_, window, cx| {
                        show.update(cx, |this, cx| this.open_tab_catalog(window, cx));
                    },
                ))
            })
    }

    /// Show the active Markdown tab's rendered document, or its source.
    ///
    /// The manual switch is the strip's control: turning the preview ON puts
    /// the source-anchored surfaces away, because a comment panel pointing at
    /// lines cannot be read over a rendered document.
    pub(crate) fn toggle_preview(&mut self, cx: &mut Context<Self>) {
        let Some(rel) = self.files.active_tab().map(|tab| tab.rel.clone()) else {
            return;
        };
        let on = !self.files.active_tab().is_some_and(|tab| tab.preview);
        if on {
            self.files.focus_note = None;
            self.files.thread_quote = None;
            self.files.comment_open = false;
        }
        self.set_preview(&rel, on, cx);
    }

    /// Put one tab into (or out of) its rendered view.
    pub(crate) fn set_preview(&mut self, rel: &Path, on: bool, cx: &mut Context<Self>) {
        let Some(index) = self.files.tab_index(rel) else {
            return;
        };
        if !renders_preview(rel) || self.files.tabs[index].preview == on {
            return;
        }
        if on && !self.refresh_render(index, cx) {
            // A document that will not render keeps its source on screen: the
            // reader can fix it there, and the pane says why it refused.
            cx.notify();
            return;
        }
        self.files.tabs[index].preview = on;
        cx.notify();
    }

    /// Re-render whichever document the tab paints.
    fn refresh_render(&mut self, index: usize, cx: &mut Context<Self>) -> bool {
        let rel = self.files.tabs[index].rel.clone();
        if files::is_svg(&rel) {
            self.refresh_svg(index, cx)
        } else {
            self.refresh_document(index, cx);
            true
        }
    }

    /// Re-rasterize an SVG from its live buffer.
    fn refresh_svg(&mut self, index: usize, cx: &mut Context<Self>) -> bool {
        let Some(editor) = self
            .files
            .tabs
            .get(index)
            .and_then(|tab| tab.editor.clone())
        else {
            return false;
        };
        let text = editor.read(cx).value().to_string();
        let rel = self.files.tabs[index].rel.clone();
        let bytes = self
            .state
            .workspace_root()
            .and_then(|root| files::resolve(&root, &rel))
            .and_then(|path| path.metadata().ok())
            .map(|meta| meta.len())
            .unwrap_or(text.len() as u64);
        match svg_preview(text.as_bytes(), bytes) {
            Some(svg) => {
                self.files.tabs[index].svg = Some(svg);
                true
            }
            None => {
                self.files.error = Some(self.state.lang.tr("right.files.svgFailed"));
                false
            }
        }
    }

    /// Re-parse a tab's document from its live buffer.
    ///
    /// The buffer, not the disk: a reader who edited the source and switched to
    /// the preview expects to read what they just wrote, and the preview must
    /// not resurrect the saved text behind their back.
    fn refresh_document(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(editor) = self
            .files
            .tabs
            .get(index)
            .and_then(|tab| tab.editor.clone())
        else {
            return;
        };
        let text = editor.read(cx).value().to_string();
        match self.files.tabs[index].document.clone() {
            Some(document) => document.update(cx, |state, cx| state.set_text(text.as_str(), cx)),
            None => {
                let document = cx.new(|cx| TextViewState::markdown(text.as_str(), cx));
                self.files.tabs[index].document = Some(document);
            }
        }
    }

    /// Raise the review card for the current selection.
    pub(crate) fn open_comment_card(&mut self, cx: &mut Context<Self>) {
        // The card quotes source lines, so a rendered Markdown tab shows its
        // source first: commenting from the preview is not a dead control.
        if let Some(rel) = self.files.active_tab().map(|tab| tab.rel.clone()) {
            if self.files.active_tab().is_some_and(|tab| tab.preview) {
                self.set_preview(&rel, false, cx);
            }
        }
        // The card is the write surface and the thread panel is the reading one,
        // so raising the card does not also open the drawer: the drawer would
        // take the code's height for a list the reader did not ask to see.
        self.files.comment_open = true;
        cx.notify();
    }

    /// One mark per comment on the editor's left edge, at the line it is
    /// pinned to.
    ///
    /// This is the browsing half of a comment: a reader scrolling a file sees
    /// where the remarks are without opening anything, and a press puts that
    /// remark into the thread panel under the code instead of covering the code
    /// it is about. Resolved remarks keep a faint mark so the history stays
    /// findable, and the mark the reader is reading grows.
    fn note_rail(&self, rel: &Path, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(tab) = self.files.active_tab() else {
            return Vec::new();
        };
        let Some(editor) = tab.editor.clone() else {
            return Vec::new();
        };
        let notes = self.files.notes.on(rel);
        if notes.is_empty() {
            return Vec::new();
        }
        // The decoration collection already tracks every commented range through
        // the buffer's own edits, so the ordinary path reads NO text: the ranges
        // it holds are the line positions, in the same order the notes were
        // synced into it. A note added without that sync (a test, a sidecar read
        // before the first frame) falls back to measuring the buffer once.
        let ranged: Vec<(usize, std::ops::Range<usize>)> = match tab.decorations.clone() {
            Some(collection) if !tab.note_marks.is_empty() => collection
                .get_ranges(cx)
                .into_iter()
                .zip(tab.note_marks.iter().copied())
                .map(|(range, index)| (index, range))
                .collect(),
            _ => {
                let text = editor.read(cx).value();
                notes
                    .iter()
                    .enumerate()
                    .filter_map(|(index, note)| {
                        let from = note.line?;
                        let to = note.end().unwrap_or(from);
                        let start = crate::notes::line_start_offset(text.as_ref(), from);
                        let end =
                            crate::notes::line_start_offset(text.as_ref(), to.saturating_add(1))
                                .min(text.len())
                                .max(start + 1);
                        (end > start).then_some((index, start..end))
                    })
                    .collect()
            }
        };
        let marks: Vec<(usize, f32, bool, usize)> = {
            let state = editor.read(cx);
            let editor_box = viewport_box(state);
            let height = f32::from(editor_box.size.height);
            if height <= 1.0 {
                return Vec::new();
            }
            let mut per_line: HashMap<usize, usize> = HashMap::new();
            ranged
                .into_iter()
                .filter_map(|(index, range)| {
                    let note = notes.get(index)?;
                    let line = note.line?;
                    let bounds = state.range_to_bounds(&range)?;
                    let (_, y) = document_origin(bounds, editor_box);
                    // Only the laid-out part of the document can carry a mark: a
                    // line above or below the viewport has no box to sit on.
                    if y < -1.0 || y > height {
                        return None;
                    }
                    let slot = per_line.entry(line).or_insert(0);
                    let column = *slot;
                    *slot += 1;
                    Some((index, y, note.resolved, column))
                })
                .collect()
        };
        let focused = self.files.focus_note;
        let jump_rel = rel.to_path_buf();
        let mut out = Vec::with_capacity(marks.len());
        for (index, y, resolved, column) in marks {
            let mut tip = self.files.notes.on(rel)[index]
                .text
                .trim()
                .replace('\n', " ");
            if tip.chars().count() > 48 {
                tip = format!("{}…", tip.chars().take(47).collect::<String>());
            }
            let color = if resolved {
                theme::border_strong()
            } else {
                theme::accent()
            };
            let wide = if focused == Some(index) { 6.0 } else { 4.0 };
            let jump = jump_rel.clone();
            let tip_text = tip.clone();
            out.push(
                div()
                    .id(SharedString::from(format!("note-mark-{index}")))
                    .debug_selector(move || format!("note-mark-{index}"))
                    .absolute()
                    .top(px(y))
                    .left(px(3.0 + column as f32 * 6.0))
                    .w(px(wide))
                    .h(px(13.0))
                    .rounded_full()
                    .bg(rgb(color))
                    .occlude()
                    .cursor_pointer()
                    .hover(|s| s.w(px(6.0)))
                    .tooltip_show_delay(std::time::Duration::from_millis(400))
                    .tooltip(move |window, cx| Tooltip::new(tip_text.clone()).build(window, cx))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.focus_comment(&jump, index, cx);
                    }))
                    .into_any_element(),
            );
        }
        out
    }

    /// The comment the reader is reading, docked under the code.
    ///
    /// The panel is a sibling of the editor and not an overlay: a card that
    /// covers the line it describes is the one thing a comment surface cannot
    /// ask a reader to accept, and a docked panel keeps every control
    /// hit-testable at the panel's narrowest width. It carries the quote, the
    /// remark, and the three things a comment is for - ask the model, resolve
    /// it, or delete it.
    fn note_thread(&self, rel: &Path, cx: &mut Context<Self>) -> Option<AnyElement> {
        let index = self.files.focus_note?;
        let note = self.files.notes.on(rel).get(index)?.clone();
        let lang = self.state.lang;
        // The excerpt is cached when the note is focused and refreshed by the
        // editor's Change event: a frame must not clone the whole file to draw
        // a three-line quote.
        let quote = match &self.files.thread_quote {
            Some((quote_rel, quote_index, text)) if quote_rel == rel && *quote_index == index => {
                text.clone()
            }
            _ => note.quote.clone(),
        };
        let quote = {
            let total = quote.lines().count();
            let shown: Vec<&str> = quote.lines().take(3).collect();
            let mut text = shown.join("\n");
            if total > shown.len() {
                text.push('…');
            }
            text
        };
        let anchor = note.anchor();
        let body = note.text.clone();
        let resolved = note.resolved;
        let sent = note.sent;
        let name = self
            .files
            .active_tab()
            .map(|tab| tab.label())
            .unwrap_or_default();
        let ask_rel = rel.to_path_buf();
        let res_rel = rel.to_path_buf();
        let del_rel = rel.to_path_buf();
        let jump_rel = rel.to_path_buf();
        let resolve_label = if resolved {
            lang.tr("right.files.commentReopen")
        } else {
            lang.tr("right.files.commentResolve")
        };
        let ask_label = lang.tr("right.files.commentAsk");
        let delete_label = lang.tr("right.files.commentDelete");
        Some(
            div()
                .debug_selector(|| String::from("files-thread"))
                .id("files-thread")
                .flex_none()
                .w_full()
                .border_t_1()
                .border_color(rgb(theme::border_soft()))
                .bg(rgb(theme::page()))
                .px(px(theme::RIGHT_PAD))
                .py(px(8.0))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(icon(
                            IconName::MessageSquare,
                            12.0,
                            if resolved {
                                theme::faint()
                            } else {
                                theme::accent()
                            },
                        ))
                        .child(txt(anchor, 11.0, theme::muted()))
                        .child(txt(name, 11.0, theme::faint()).truncate())
                        .child(div().flex_1())
                        .when(resolved, |row| {
                            row.child(
                                txt(lang.tr("right.files.commentResolved"), 11.0, theme::faint())
                                    .font_weight(FontWeight::MEDIUM),
                            )
                        })
                        .when(sent, |row| {
                            row.child(
                                txt(lang.tr("right.files.commentSent"), 11.0, theme::faint())
                                    .font_weight(FontWeight::MEDIUM),
                            )
                        })
                        .child(
                            div()
                                .id("files-thread-close")
                                .debug_selector(|| String::from("files-thread-close"))
                                .w(px(18.0))
                                .h(px(18.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.0))
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(theme::hover())))
                                .child(icon(IconName::X, 11.0, theme::faint()))
                                .on_click(cx.listener(|this, _event, _window, cx| {
                                    this.files.focus_note = None;
                                    cx.notify();
                                })),
                        ),
                )
                .when(!quote.trim().is_empty(), |panel| {
                    panel.child(
                        div()
                            .id("files-thread-quote")
                            .debug_selector(|| String::from("files-thread-quote"))
                            .w_full()
                            .px(px(8.0))
                            .py(px(5.0))
                            .rounded(px(6.0))
                            .bg(rgb(theme::hover()))
                            .cursor_pointer()
                            .child(
                                txt(quote, 11.0, theme::muted())
                                    .font_family(theme::CODE_FONT)
                                    .line_height(px(16.0))
                                    .max_h(px(48.0))
                                    .overflow_hidden(),
                            )
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.reveal_comment(&jump_rel, index, cx);
                            })),
                    )
                })
                .child(txt(body, 12.5, theme::text()).line_height(px(18.0)))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .id("files-thread-ask")
                                .debug_selector(|| String::from("files-thread-ask"))
                                .h(px(26.0))
                                .px(px(10.0))
                                .rounded(px(theme::RADIUS_SM))
                                .bg(rgb(theme::accent()))
                                .cursor_pointer()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(4.0))
                                .child(icon(IconName::ArrowUp, 12.0, theme::page()))
                                .child(
                                    txt(ask_label, theme::CHIP_SIZE, theme::page())
                                        .font_weight(FontWeight::MEDIUM),
                                )
                                .on_click(cx.listener(move |this, _event, window, cx| {
                                    this.ask_comment(&ask_rel, index, window, cx);
                                })),
                        )
                        .child(
                            div()
                                .id("files-thread-resolve")
                                .debug_selector(|| String::from("files-thread-resolve"))
                                .h(px(26.0))
                                .px(px(8.0))
                                .rounded(px(theme::RADIUS_SM))
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(theme::hover())))
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(4.0))
                                .child(icon(
                                    if resolved {
                                        IconName::Circle
                                    } else {
                                        IconName::Check
                                    },
                                    12.0,
                                    theme::muted(),
                                ))
                                .child(txt(resolve_label, theme::CHIP_SIZE, theme::muted()))
                                .on_click(cx.listener(move |this, _event, _window, cx| {
                                    this.resolve_comment(&res_rel, index, cx);
                                })),
                        )
                        .child(
                            div()
                                .id("files-thread-delete")
                                .debug_selector(|| String::from("files-thread-delete"))
                                .h(px(26.0))
                                .px(px(8.0))
                                .rounded(px(theme::RADIUS_SM))
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(theme::hover())))
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(4.0))
                                .child(icon(IconName::Trash, 12.0, theme::muted()))
                                .child(txt(delete_label, theme::CHIP_SIZE, theme::muted()))
                                .on_click(cx.listener(move |this, _event, _window, cx| {
                                    this.files.focus_note = None;
                                    this.delete_note(&del_rel, index, cx);
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The inline review card, shaped like the reference's: a titled box over the
    /// document holding one growing field, the anchor it will carry, and Cancel /
    /// Comment. Writing a note is a mode the card makes obvious; the strip's
    /// Comments control is its door.
    /// The little + that follows a line selection, and the press that opens the
    /// comment card: the reference puts the affordance where the reader's hand
    /// already is, instead of asking them to find a toolbar first.
    fn comment_plus(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let editor = self.files.active_tab()?.editor.clone()?;
        let state = editor.read(cx);
        let range = state.selected_range();
        if range.start >= range.end {
            return None;
        }
        // The control rides a line of the selection that the viewport actually
        // holds, so it follows the marks through a scroll instead of vanishing
        // with them. `anchor_line` names that line; its own slice of the
        // selection is the box to centre on, and `document_point` converts that
        // box's window coordinates into the editor's.
        let text = state.value();
        let rows = state.visible_row_range()?;
        let line = anchor_line(text.as_ref(), &range, &rows)?;
        let start = crate::notes::line_start_offset(text.as_ref(), line);
        let line_end = text.as_ref()[start..]
            .find('\n')
            .map(|at| start + at)
            .unwrap_or(text.len());
        // The part of the selection on that line. A selection that stops at the
        // line break leaves an empty slice there, so the line's first character
        // becomes the anchor instead of a zero-width box.
        let from = range.start.max(start).min(line_end);
        let to = range.end.min(line_end).max(from + 1).min(text.len());
        let bounds = state
            .range_to_bounds(&(from..to))
            .or_else(|| state.range_to_bounds(&(start..(start + 1).min(text.len()))))?;
        // The element's VIEWPORT box (before the editor's own scroll is folded
        // into it), which is what the overlay's parent is measured from. A box
        // with no size yet has no frame to place a control in.
        let editor = viewport_box(state);
        if f32::from(editor.size.width) <= 1.0 || f32::from(editor.size.height) <= 1.0 {
            return None;
        }
        let (left, top) = document_point(bounds, editor, 20.0);
        Some(
            div()
                .debug_selector(|| String::from("files-comment-plus"))
                .id("files-comment-plus")
                .absolute()
                .top(px(top))
                .left(px(left))
                .w(px(20.0))
                .h(px(20.0))
                .rounded_full()
                .bg(rgb(theme::accent()))
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.opacity(0.85))
                .child(icon(IconName::Plus, 12.0, theme::page()))
                // The press routes: this is the control a reader actually presses,
                // and doing so is what surfaced the duplicate-field crash. The
                // harness cannot drive it - a synthetic press through the focused
                // editor lands on the editor - so the tests call the action it
                // fires and assert the control is on screen and on the selection.
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.open_comment_card(cx);
                }))
                .into_any_element(),
        )
    }
    fn comment_card(&self, rel: &Path, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        // The scope is the same fact pinning reads, so the card cannot label a
        // range the note will not carry.
        let (line, to, quote) = self.note_scope(cx);
        let anchor = match (line, to) {
            (Some(from), Some(to)) if to > from => lang
                .tr("right.files.commentLines")
                .replace("{from}", &from.to_string())
                .replace("{to}", &to.to_string()),
            (Some(from), _) => lang
                .tr("right.files.commentLine")
                .replace("{line}", &from.to_string()),
            (None, _) => lang.tr("right.files.commentFile"),
        };
        let quote = {
            let mut lines: Vec<&str> = quote.lines().take(3).collect();
            let more = quote.lines().count() > lines.len();
            let mut text = lines.join("\n");
            if more {
                text.push('…');
            }
            lines.clear();
            text
        };

        // The card hangs directly under the lines being commented, the way the
        // reference puts it: `range_to_bounds` gives the selection's own box, so
        // the card starts one gap below its bottom edge. The box is in window
        // space, so it is converted into the editor's before it is used as a
        // `top()` - the same conversion the + uses.
        let selection_bottom = self
            .files
            .active_tab()
            .and_then(|tab| tab.editor.clone())
            .and_then(|editor| {
                let state = editor.read(cx);
                let selection = state.range_to_bounds(&state.selected_range())?;
                let editor_box = viewport_box(state);
                let height = f32::from(editor_box.size.height);
                if height <= 1.0 {
                    return None;
                }
                // The BOTTOM edge, from the normalised corners: the kit's box
                // has a negative height when the range ends on a shorter line,
                // and adding that put the card above the lines it describes.
                let top = selection.origin.y.min(selection.bottom_right().y);
                let bottom = selection.origin.y.max(selection.bottom_right().y);
                let below = f32::from(bottom - editor_box.origin.y) + 8.0;
                // The card must not be cut by the pane it is drawn in: a
                // selection near the bottom flips the card above itself instead
                // of opening a surface whose buttons sit off screen.
                Some(if below + CARD_ROOM <= height {
                    below
                } else {
                    (f32::from(top - editor_box.origin.y) - CARD_ROOM - 8.0).max(4.0)
                })
            });
        let send_rel = rel.to_path_buf();
        let can_send = !self.files.note_text.read(cx).value().trim().is_empty();
        div()
            .debug_selector(|| String::from("files-comment-card"))
            .absolute()
            // Over the document's text area, so it must take the pointer ahead of
            // the editor: without this the focused input ate every press and the
            // card's own buttons never fired.
            .occlude()
            // Beside the lines when the editor can say where they are, and at the
            // top otherwise (a tab with no editor, or before the first layout).
            .when_some(selection_bottom, |card, top| card.top(px(top)))
            .when(selection_bottom.is_none(), |card| card.top(px(20.0)))
            // Left and right bounds instead of a fixed width: a 460px card anchored
            // at the panel's left edge ran past the window, which put its buttons
            // off screen - the probe put Comment at x=1850 in a 1640 window.
            .left(px(theme::RIGHT_PAD + 24.0))
            .right(px(16.0))
            .flex()
            .items_start()
            .child(
                div()
                    .w_full()
                    .max_w(px(460.0))
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(rgb(theme::border()))
                    .bg(rgb(theme::page()))
                    .shadow_lg()
                    .p(px(14.0))
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(
                        txt(lang.tr("right.files.commentTitle"), 13.0, theme::text())
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    // What the reader is commenting on, shown before they type:
                    // a remark is about a place, and the quote is that place.
                    .when(!quote.is_empty(), |card| {
                        card.child(
                            div()
                                .debug_selector(|| String::from("files-comment-quote"))
                                .w_full()
                                .px(px(8.0))
                                .py(px(5.0))
                                .rounded(px(6.0))
                                .bg(rgb(theme::hover()))
                                .child(
                                    txt(quote, 11.0, theme::muted())
                                        .font_family(theme::CODE_FONT)
                                        .line_height(px(16.0)),
                                ),
                        )
                    })
                    .child(
                        div()
                            .debug_selector(|| String::from("files-comment-field"))
                            .h(px(76.0))
                            .px(px(8.0))
                            .py(px(6.0))
                            .rounded(px(theme::RADIUS_SM))
                            .border_1()
                            .border_color(rgb(theme::border()))
                            .child(Textarea::new(&self.files.note_text).appearance(false)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(txt(anchor, 12.0, theme::faint()))
                            .child(div().flex_1())
                            .child(
                                div()
                                    .id("files-comment-cancel")
                                    .debug_selector(|| String::from("files-comment-cancel"))
                                    .px(px(10.0))
                                    .h(px(28.0))
                                    .flex()
                                    .items_center()
                                    .rounded(px(theme::RADIUS_SM))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgb(theme::hover())))
                                    .child(txt(
                                        lang.tr("right.files.commentCancel"),
                                        12.0,
                                        theme::text(),
                                    ))
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.files.comment_open = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("files-comment-add")
                                    .debug_selector(|| String::from("files-comment-add"))
                                    .px(px(12.0))
                                    .h(px(28.0))
                                    .flex()
                                    .items_center()
                                    .rounded(px(theme::RADIUS_SM))
                                    .cursor_pointer()
                                    .bg(rgb(if can_send {
                                        theme::accent()
                                    } else {
                                        theme::border()
                                    }))
                                    .child(txt(
                                        lang.tr("right.files.commentAction"),
                                        12.0,
                                        theme::page(),
                                    ))
                                    .on_click(cx.listener(move |this, _event, window, cx| {
                                        // Comment pins the remark and asks about
                                        // it in one press. Only the remarks that have
                                        // not gone out yet travel, and the send goes
                                        // through the composer's own submit path, so
                                        // it queues behind a running turn.
                                        this.pin_note(window, cx);
                                        this.send_unsent_notes(&send_rel, window, cx);
                                        this.files.comment_open = false;
                                        this.files.notes_open = false;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
    }
    /// The drawer that lists a file's comments, in the order they were pinned.
    ///
    /// It is a list and not a second place to write: the card over the code is
    /// where a comment is composed, so the drawer's job is to show what is
    /// there, to move the reader to each remark, and to send the open ones to
    /// the model as one review.
    fn files_notes(&self, rel: &Path, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        let notes = self.files.notes.on(rel);
        let open = notes.iter().filter(|note| !note.resolved).count();
        let resolved = notes.len().saturating_sub(open);
        let send_rel = rel.to_path_buf();
        let send = open > 0;
        let counts = lang
            .tr("right.files.commentOpen")
            .replace("{open}", &open.to_string())
            .replace("{resolved}", &resolved.to_string());
        let rows = notes
            .iter()
            .enumerate()
            .map(|(index, note)| {
                let anchor = note.anchor();
                let debug_anchor = anchor.clone();
                let text = note.text.clone();
                let resolved = note.resolved;
                let sent = note.sent;
                let jump_rel = rel.to_path_buf();
                let res_rel = rel.to_path_buf();
                let del_rel = rel.to_path_buf();
                let del_anchor = anchor.clone();
                let row_id = SharedString::from(format!("files-note-row-{index}"));
                div()
                    .debug_selector(move || format!("files-note-{debug_anchor}"))
                    .id(row_id)
                    .w_full()
                    .px(px(8.0))
                    .py(px(5.0))
                    .rounded(px(theme::RADIUS_SM))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(theme::hover())))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.focus_comment(&jump_rel, index, cx);
                    }))
                    .child(txt(anchor, 11.0, theme::muted()).flex_none().w(px(46.0)))
                    .child(
                        txt(text, theme::SIDEBAR_SIZE, theme::text())
                            .truncate()
                            .flex_1()
                            .min_w_0()
                            .when(resolved, |line| line.opacity(0.55)),
                    )
                    .when(sent, |row| {
                        row.child(
                            txt(lang.tr("right.files.commentSent"), 10.0, theme::faint())
                                .flex_none(),
                        )
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!("files-note-resolve-{index}")))
                            .debug_selector(move || format!("files-note-resolve-{index}"))
                            .w(px(20.0))
                            .h(px(20.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(theme::hover())))
                            .child(icon(
                                if resolved {
                                    IconName::Circle
                                } else {
                                    IconName::CircleCheck
                                },
                                13.0,
                                if resolved {
                                    theme::faint()
                                } else {
                                    theme::success()
                                },
                            ))
                            .on_click(cx.listener(
                                move |this, event: &gpui::ClickEvent, _window, cx| {
                                    cx.stop_propagation();
                                    let _ = event;
                                    this.resolve_comment(&res_rel, index, cx);
                                },
                            )),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("files-note-del-{index}")))
                            .debug_selector(move || format!("files-note-del-{del_anchor}"))
                            .w(px(20.0))
                            .h(px(20.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(theme::hover())))
                            .child(icon(IconName::Trash, 12.0, theme::faint()))
                            .on_click(cx.listener(
                                move |this, event: &gpui::ClickEvent, _window, cx| {
                                    cx.stop_propagation();
                                    let _ = event;
                                    this.delete_note(&del_rel, index, cx);
                                },
                            )),
                    )
            })
            .collect::<Vec<_>>();
        div()
            .debug_selector(|| String::from("files-notes"))
            .flex_none()
            .w_full()
            .px(px(theme::RIGHT_PAD))
            .pt(px(6.0))
            .pb(px(4.0))
            .border_b_1()
            .border_color(rgb(theme::border_soft()))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        txt(lang.tr("right.files.notes"), 12.0, theme::text())
                            .font_weight(FontWeight::MEDIUM),
                    )
                    .child(txt(counts, 11.0, theme::faint()))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("files-note-add")
                            .debug_selector(|| String::from("files-note-add"))
                            .h(px(22.0))
                            .px(px(7.0))
                            .rounded(px(theme::RADIUS_SM))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(theme::hover())))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(3.0))
                            .child(icon(IconName::Plus, 12.0, theme::muted()))
                            .child(txt(lang.tr("right.files.noteAdd"), 11.0, theme::muted()))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.open_comment_card(cx);
                            })),
                    )
                    .when(send, |row| {
                        row.child(
                            div()
                                .id("files-note-send")
                                .debug_selector(|| String::from("files-note-send"))
                                .h(px(22.0))
                                .px(px(7.0))
                                .rounded(px(theme::RADIUS_SM))
                                .bg(rgb(theme::accent()))
                                .cursor_pointer()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(3.0))
                                .child(icon(IconName::ArrowUp, 11.0, theme::page()))
                                .child(
                                    txt(lang.tr("right.files.noteSend"), 11.0, theme::page())
                                        .font_weight(FontWeight::MEDIUM),
                                )
                                .on_click(cx.listener(move |this, _event, window, cx| {
                                    this.send_review_to_chat(&send_rel, window, cx);
                                })),
                        )
                    })
                    .child(
                        div()
                            .id("files-notes-close")
                            .debug_selector(|| String::from("files-notes-close"))
                            .w(px(20.0))
                            .h(px(20.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(theme::hover())))
                            .child(icon(IconName::X, 11.0, theme::faint()))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.files.notes_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                // A long review scrolls inside its own 168px plate instead of
                // taking the document's height: the drawer is a roster, not the
                // surface the reader is working on.
                div()
                    .id("files-notes-list")
                    .debug_selector(|| String::from("files-notes-list"))
                    .w_full()
                    .max_h(px(168.0))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .when(notes.is_empty(), |list| {
                        list.child(div().px(px(8.0)).py(px(4.0)).child(txt(
                            lang.tr("right.files.commentEmpty"),
                            11.0,
                            theme::faint(),
                        )))
                    })
                    .children(rows),
            )
    }

    /// The rendered document a Markdown tab shows instead of its source.
    ///
    /// The kit's markdown view is the same renderer the transcript uses, so a
    /// document reads here exactly as it reads in a reply. It scrolls on its
    /// own and is selectable, and it is read-only by contract: the source stays
    /// the editable truth behind the strip's preview control.
    fn files_preview(&self, document: &Entity<TextViewState>) -> Div {
        div()
            .debug_selector(|| String::from("files-preview"))
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .debug_selector(|| String::from("files-preview-body"))
                    .id("files-preview-body")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_y_scroll()
                    .px(px(theme::RIGHT_PAD))
                    .py(px(10.0))
                    .child(
                        TextView::new(document)
                            .style(self.markdown_style())
                            .selectable(true)
                            .w_full(),
                    ),
            )
    }

    /// The rasterized picture an SVG tab previews.
    ///
    /// The source stays one switch away: an SVG is text, and the editor is where
    /// it is edited, but the pane opens on the drawing because that is what a
    /// reader opening a picture asked for.
    fn files_svg(&self, svg: &SvgPreview) -> Div {
        let caption = format!(
            "{} × {} · {}",
            svg.width,
            svg.height,
            size_label(svg.bytes, false)
        );
        div()
            .debug_selector(|| String::from("files-svg"))
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        img(ImageSource::Render(svg.frame.clone()))
                            .max_w(relative(1.0))
                            .max_h(relative(0.88)),
                    )
                    .child(
                        txt(caption, 11.0, theme::faint())
                            .debug_selector(|| String::from("files-image-info")),
                    ),
            )
    }

    /// The editor over the active tab: the file's own header, then the body.
    /// Absent when no tab is open, which leaves the tree the full width.
    ///
    /// The surface is the editor and nothing else - the tab chip already names
    /// the file, the dirty dot rides the chip, and cmd-s saves - so the bar
    /// that used to repeat the path and carry a save button is gone, and the
    /// action handler below is the whole save affordance.
    fn files_editor(&self, root: &Path, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        let Some(open) = self.files.active_tab() else {
            return div()
                .debug_selector(|| String::from("files-editor"))
                .flex_1()
                .min_h_0()
                .w_full();
        };
        let rel = open.rel.clone();
        let save_rel = rel.clone();
        let shell = cx.entity();
        div()
            .debug_selector(|| String::from("files-editor"))
            .flex_1()
            .min_h_0()
            .w_full()
            .relative()
            .flex()
            .flex_col()
            .overflow_hidden()
            // cmd-s, dispatched from the focused editor, bubbles to here.
            .on_action(move |_event: &SaveFile, _window, cx| {
                shell.update(cx, |this, cx| {
                    this.save_file(&save_rel, cx);
                });
            })
            // The notes drawer, over the top of the document when open.
            .when(self.files.notes_open, |editor| {
                editor.child(self.files_notes(&rel, cx))
            })
            .child(match (&open.editor, open.kind) {
                // A rendered tab paints its document instead of its source:
                // an SVG is a picture, a Markdown file is a parsed view.
                (Some(_), _) if open.preview && open.svg.is_some() => {
                    self.files_svg(open.svg.as_ref().expect("the svg preview"))
                }
                (Some(_), _) if open.preview && open.document.is_some() => {
                    self.files_preview(open.document.as_ref().expect("the preview document"))
                }
                (Some(editor), _) => div()
                    .debug_selector(|| String::from("files-editor-body"))
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(
                        div()
                            .debug_selector(|| String::from("files-editor-frame"))
                            .id("files-editor-frame")
                            .size_full()
                            .flex_1()
                            .min_h_0()
                            .px(px(theme::RIGHT_PAD))
                            .py(px(6.0))
                            .border_1()
                            .border_color(rgb(theme::border_soft()))
                            // The kit editor draws its own bordered surface and
                            // sizes it to the content, so a SHORT file was a short
                            // box with the rest of the pane empty. Stretching it is
                            // what makes the pane read as a document.
                            .child(
                                div()
                                    .debug_selector(|| String::from("files-editor-fill"))
                                    // The frame around this is a plain block, so a
                                    // flex child here had nothing to grow in and
                                    // collapsed to the editor's own content height
                                    // (58px for a three-line file). A full height
                                    // is the fact, not a flex share.
                                    .h_full()
                                    .min_h_0()
                                    .w_full()
                                    .flex()
                                    .flex_col()
                                    // The parent every document-anchored overlay is
                                    // measured against: the kit answers text boxes in
                                    // WINDOW space, so the overlay container has to be
                                    // the editor box for `document_point` subtraction
                                    // to be the whole conversion.
                                    .relative()
                                    .child(Editor::new(editor).h_full().w_full())
                                    // Pinned notes, the + that follows a selection and
                                    // the review card come LAST so they paint over the
                                    // document: added before the editor they were drawn
                                    // UNDER it, and the editor swallowed their presses.
                                    .children(self.note_rail(&rel, cx))
                                    .children(self.comment_plus(cx))
                                    .when(self.files.comment_open, |fill| {
                                        fill.child(self.comment_card(&rel, cx))
                                    }),
                            ),
                    ),
                (None, Kind::Image) => {
                    let mut pane = div()
                        .debug_selector(|| String::from("files-image"))
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .overflow_hidden();
                    match files::resolve(root, &rel) {
                        Some(path) => {
                            let caption = image_caption(&path);
                            let mut stack = div()
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap(px(8.0))
                                .child(img(path).max_w(relative(1.0)).max_h(relative(0.88)));
                            if let Some(caption) = caption {
                                stack = stack.child(
                                    txt(caption, 11.0, theme::faint())
                                        .debug_selector(|| String::from("files-image-info")),
                                );
                            }
                            pane = pane.child(stack);
                        }
                        // The tab's path must resolve inside the root; if it no
                        // longer does, say so rather than handing the image pane
                        // an empty path it silently draws nothing for.
                        None => {
                            pane = pane.child(
                                txt(
                                    lang.tr("right.files.outside"),
                                    theme::CHIP_SIZE,
                                    theme::faint(),
                                )
                                .debug_selector(|| String::from("files-image-outside")),
                            );
                        }
                    }
                    pane
                }
                (None, _) => div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(8.0))
                    .px(px(theme::RIGHT_PAD * 2.0))
                    .child(icon(IconName::Lock, 20.0, theme::faint()))
                    .child(
                        txt(
                            lang.tr("right.files.opaque"),
                            theme::CHIP_SIZE,
                            theme::faint(),
                        )
                        .text_align(TextAlign::Center)
                        .line_height(px(18.0)),
                    ),
            })
            // The focused comment is a dock under the code, so reading a remark
            // and reading the line it is about happen at the same time.
            .when_some(self.note_thread(&rel, cx), |column, thread| {
                column.child(thread)
            })
    }
}

/// The last press inside the page, so its release can carry the same count.
///
/// The count itself is the PLATFORM's (`MouseDownEvent::click_count`, which is
/// AppKit's own `clickCount`): it is measured in window coordinates against
/// the real double-click interval and point, so a viewport resize between the
/// two presses cannot break it. The pane used to time the count itself in PAGE
/// coordinates, and MEASURED, a resize that moved the page point from
/// (500, 350) to (240, 502) between two presses reset the count to one - an
/// ordinary double click never became a `dblclick`. The stored button keeps a
/// right press from continuing a left press's count.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BrowserClick {
    /// The button this count belongs to.
    button: MouseButton,
    /// 1 for a single click, 2 for a double, 3 for a triple.
    count: u32,
}

/// The least time between two viewport overrides the pane applies.
///
/// A resize is a CDP round trip, so a pane drag must not send one per frame.
/// 150 ms reflows the page at a rate the eye reads as continuous during a
/// drag and catches the final size within one window after it stops; the
/// override itself runs off the UI thread.
pub(crate) const BROWSER_RESIZE_EVERY: std::time::Duration = std::time::Duration::from_millis(150);

impl BrowserClick {
    /// The count a press of `button` carries, given the platform's count.
    ///
    /// A press of a different button starts a new sequence at one, because a
    /// right press must not turn the page's next left click into a double. The
    /// platform's count is bounded at three: the page's own events only name
    /// 1, 2 and 3.
    pub(crate) fn press(
        previous: Option<&BrowserClick>,
        button: MouseButton,
        click_count: usize,
    ) -> Self {
        let continues = previous.is_some_and(|previous| previous.button == button);
        let count = if continues {
            click_count.clamp(1, 3) as u32
        } else {
            1
        };
        Self { button, count }
    }

    /// The count this press carried.
    pub(crate) fn count(&self) -> u32 {
        self.count
    }
}

/// The CDP button for a GPUI button, or `None` for a navigation button.
///
/// The pane registers the three pointer buttons; a navigation button (back or
/// forward) has no page event, and forwarding it as a left click would put a
/// press on the page the reader never made.
fn pointer_button(button: MouseButton) -> Option<PointerButton> {
    match button {
        MouseButton::Left => Some(PointerButton::Left),
        MouseButton::Right => Some(PointerButton::Right),
        MouseButton::Middle => Some(PointerButton::Middle),
        MouseButton::Navigate(_) => None,
    }
}

/// Map a window point to PAGE coordinates inside `bounds`.
///
/// `clamp` is the captured-pointer case: a release that lands outside the
/// frame is still a release the page must see, at the frame's edge, so the
/// gesture ends where the pointer left the page rather than staying pressed
/// forever. Without it an outside point is `None` (the press was not the
/// page's).
fn page_point(
    bounds: Bounds<Pixels>,
    viewport: (u32, u32),
    position: Point<Pixels>,
    clamp: bool,
) -> Option<(f32, f32)> {
    let width = bounds.size.width.as_f32().max(1.0);
    let height = bounds.size.height.as_f32().max(1.0);
    let mut x = position.x.as_f32() - bounds.origin.x.as_f32();
    let mut y = position.y.as_f32() - bounds.origin.y.as_f32();
    if clamp {
        x = x.clamp(0.0, (width - 1.0).max(0.0));
        y = y.clamp(0.0, (height - 1.0).max(0.0));
    } else if x < 0.0 || y < 0.0 || x > width || y > height {
        return None;
    }
    Some((
        x * viewport.0 as f32 / width,
        y * viewport.1 as f32 / height,
    ))
}

/// The CDP modifier mask for a keystroke.
///
/// The protocol's bits are Alt 1, Ctrl 2, Meta 4, Shift 8. The pane forwards
/// the physical modifiers it received, which is why Cmd on macOS becomes Meta
/// and Ctrl elsewhere becomes Ctrl without a platform branch here.
fn chord_modifiers(modifiers: &gpui::Modifiers) -> u32 {
    let mut bits = 0;
    if modifiers.alt {
        bits |= crate::browser::engine::cdp::ALT;
    }
    if modifiers.control {
        bits |= crate::browser::engine::cdp::CTRL;
    }
    if modifiers.platform {
        bits |= crate::browser::engine::cdp::META;
    }
    if modifiers.shift {
        bits |= crate::browser::engine::cdp::SHIFT;
    }
    bits
}

/// How much further a mouse notch travels than a trackpad's own delta.
///
/// The same rule and the same number as the transcript's wheel (`MOUSE_GAIN`):
/// a wheel sends whole LINES, one coarse detent, while a trackpad sends precise
/// pixels that already carry momentum. Gaining the coarse one makes a notch a
/// comfortable step; gaining the precise one would fight the hardware.
const BROWSER_WHEEL_GAIN: f32 = 2.0;

/// One wheel event's page point and deltas, in CDP's own sign.
///
/// GPUI hands a delta the way its own scroller consumes it: positive moves the
/// CONTENT down, which is the reader scrolling UP. CDP's `mouseWheel` is the
/// opposite - a positive `deltaY` scrolls the page DOWN - so forwarding the
/// window's number unchanged scrolls every page backwards. Both axes are
/// negated here, which is why this is a function a test can pin rather than a
/// sign in the middle of a handler.
///
/// The point is CLAMPED into the frame: a wheel delivered at the pane's edge is
/// still a wheel over the page, and CDP would otherwise aim it at nothing.
fn page_wheel(
    bounds: Bounds<Pixels>,
    viewport: (u32, u32),
    position: Point<Pixels>,
    delta: &ScrollDelta,
    line_height: Pixels,
) -> Option<(f32, f32, f32, f32)> {
    let (x, y) = page_point(bounds, viewport, position, true)?;
    let gain = if delta.precise() {
        1.0
    } else {
        BROWSER_WHEEL_GAIN
    };
    let pixels = delta.pixel_delta(line_height);
    Some((x, y, -pixels.x.as_f32() * gain, -pixels.y.as_f32() * gain))
}

/// The engine's name for a named (non-text) key.
///
/// Modifiers are deliberately NOT folded into the name: the pane has the
/// physical mask and [`crate::browser::engine::Engine::press_with`] carries it,
/// so Shift+Arrow extends a selection and Cmd/Ctrl+Arrow jumps a line the way
/// they do in a browser. `None` means the key's meaning is its text.
fn named_key(key: &str) -> Option<&'static str> {
    Some(match key {
        "enter" => "enter",
        "tab" => "tab",
        "escape" => "escape",
        "up" => "arrowup",
        "down" => "arrowdown",
        "left" => "arrowleft",
        "right" => "arrowright",
        "backspace" => "backspace",
        "delete" => "delete",
        "home" => "home",
        "end" => "end",
        "pageup" => "pageup",
        "pagedown" => "pagedown",
        _ => return None,
    })
}

/// The text a keystroke carries, when it is text the page should receive.
///
/// Three rules, and each is a defect without it:
///
/// - Control and Cmd chords are accelerators, not text. The ones the pane owns
///   have already been delivered as chords; the rest belong to the application.
/// - Alt is usually an accelerator too (`Option`+`f` is a menu key on many
///   layouts), so an Option chord whose character is the key itself is not
///   text.
/// - Alt that PRODUCES a different character is composition - macOS `Option`+`a`
///   is `a` with the character `å` - and dropping it means no Option-composed
///   character can be typed into a page at all.
fn typed_text(keystroke: &Keystroke) -> Option<&str> {
    if keystroke.modifiers.control || keystroke.modifiers.platform {
        return None;
    }
    let text = keystroke.key_char.as_deref()?;
    if text.is_empty() || text.chars().all(char::is_control) {
        return None;
    }
    if keystroke.modifiers.alt && keystroke.key == text {
        return None;
    }
    Some(text)
}

/// A small zoom control: a 28px clickable cell with one glyph.
fn zoom_button(
    id: &'static str,
    label: &'static str,
    cx: &mut Context<Shell>,
    action: impl Fn(&mut Shell) + 'static,
) -> AnyElement {
    let action = std::sync::Arc::new(action);
    div()
        .id(id)
        .debug_selector(move || id.to_string())
        .flex()
        .items_center()
        .justify_center()
        .w(px(28.0))
        .h(px(28.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(theme::hover())))
        .child(txt(label, 14.0, theme::text()))
        .on_click(cx.listener(move |this, _event, _window, cx| {
            action(this);
            cx.notify();
        }))
        .into_any_element()
}

/// Handle the editing chords the pane owns: Cmd/Ctrl+C, +V, +X, +A, +Z and
/// +F.
///
/// Returns whether the chord belonged to the page, so the caller stops before
/// treating the letter as text. This is the ONLY place bsh touches the
/// clipboard: paste reads it because the reader pressed paste and hands the
/// string to the page's own paste event, and copy/cut write the page's own
/// selection back. Nothing is injected at any other moment.
fn browser_shortcut(
    keystroke: &Keystroke,
    engine: &crate::browser::engine::Engine,
    cx: &mut Context<Shell>,
) -> bool {
    let modifiers = keystroke.modifiers;
    if !modifiers.secondary(cx) || modifiers.alt {
        return false;
    }
    let bits = chord_modifiers(&modifiers);
    match keystroke.key.as_str() {
        "c" => {
            if let Ok(Some(text)) = engine.copy(bits) {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            true
        }
        "x" => {
            if let Ok(Some(text)) = engine.cut(bits) {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            true
        }
        "v" => {
            let text = cx
                .read_from_clipboard()
                .and_then(|item| item.text())
                .unwrap_or_default();
            let _ = engine.paste(&text, bits);
            true
        }
        "a" => {
            // Cmd-Shift-A is the application's Archive session binding; leave
            // it alone rather than turning it into a second select-all.
            if modifiers.shift {
                return false;
            }
            let _ = engine.select_all(bits, true);
            true
        }
        "z" => {
            let _ = engine.undo(bits, true);
            true
        }
        "f" => {
            // No find bar exists in this pane; the page's own Cmd-F handler is
            // what receives the chord.
            let _ = engine.find(bits, true);
            true
        }
        _ => false,
    }
}

impl Shell {
    /// Fold or unfold the panel, the way the header's left control folds the
    /// sidebar. A fold keeps the pane, so reopening lands where the reader was.
    pub(crate) fn toggle_right_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.right.open = !self.right.open;
        if self.right.open {
            // Reopening on the terminal has to bring the native surface back:
            // a tab switch hid it, and nothing else paints it again.
            if self.right.tab == Some(RightTab::Terminal) {
                self.ensure_terminal_view(window, cx);
            }
        } else {
            self.hide_terminal_view(cx);
        }
        cx.notify();
    }

    /// Whether the terminal's native surface should be on screen this frame.
    ///
    /// Three facts decide it: the panel is open on the Terminal tab, and no
    /// full-window overlay (the settings dialog, the image lightbox) is up.
    /// The surface is a layer of the window's view, so an overlay would be
    /// painted UNDER it if the terminal stayed visible.
    pub(crate) fn terminal_surface_wanted(&self) -> bool {
        self.right.open
            && self.right.tab == Some(RightTab::Terminal)
            && self.state.overlay == crate::state::Overlay::None
            && self.image_lightbox.is_none()
    }

    /// Show `tab` in the panel, taking the terminal's native layer down when
    /// the body becomes something else.
    ///
    /// Every open path except `open_right_tab` (which may also have to spawn
    /// the surface) goes through this: the terminal is a layer of the window's
    /// view, so it has to be hidden explicitly or it keeps painting over the
    /// pane that replaced it.
    /// Record one panel opening in the launcher's automatic recent list.
    fn record_panel_recent(&mut self, tab: RightTab) {
        let label = self.state.lang.tr(tab.key());
        let detail = self.state.lang.tr(tab.description_key());
        self.state.record_launcher_recent(
            crate::recent::RecentKind::Panel,
            tab.slug(),
            label,
            detail,
            None,
        );
    }

    pub(crate) fn show_right_tab(&mut self, tab: RightTab, cx: &mut Context<Self>) {
        self.right.open = true;
        self.right.tab = Some(tab);
        if !self.right.open_panes.contains(&tab) {
            self.right.open_panes.push(tab);
        }
        self.record_panel_recent(tab);
        // Choosing a tab is also leaving the overview, the way selecting a tab
        // in Finder's own catalogue puts the window back.
        self.right.catalog_open = false;
        if tab != RightTab::Terminal {
            self.hide_terminal_view(cx);
        }
    }

    /// Bring one already-open tab to the front.
    ///
    /// The tab press path: unlike opening a pane for the first time, this never
    /// spawns a terminal or pre-warms a browser; it only restores the surface
    /// the tab owns. A hidden terminal view is a window layer, so it has to be
    /// made visible explicitly when its tab is chosen.
    pub(crate) fn activate_pane(&mut self, tab: RightTab, cx: &mut Context<Self>) {
        self.show_right_tab(tab, cx);
        if tab == RightTab::Terminal {
            if let Some(view) = self.right.terminal_view.clone() {
                view.update(cx, |view, _| view.set_visible(true));
            }
        }
        cx.notify();
    }

    /// Reveal or hide the seam's bar.
    ///
    /// The pointer moving onto the strip turns it on, and the column's own move
    /// turns it off again - the same pair the content column's handles use.
    pub(crate) fn hover_right_seam(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.right_hover != on {
            self.right_hover = on;
            cx.notify();
        }
    }

    /// The right panel column width for this frame, resolved from the reader's
    /// dragged preference against the space the row has left, the same way the
    /// sidebar's is.
    pub(crate) fn right_panel_w(&self) -> f32 {
        // The panel shares the row with the sidebar and the chat, so the space
        // it may claim is the window less the sidebar - not the window. The
        // reference's own `rightPanel: cols.right` and `sidebar: cols.sidebar`
        // sum to the same quantity: the share is a share of the row.
        let taken = if self.sidebar_folded() {
            0.0
        } else {
            self.sidebar_w()
        };
        let available = (self.viewport_w - taken).max(theme::RIGHT_PANEL_MIN);
        if self.right.maximized {
            // Every pixel the sidebar is not using: the maximised panel is the
            // reader saying "this is the thing I am working in", and the header
            // still carries the control that gives the space back.
            return available;
        }
        theme::right_panel_width_at(available, self.right.user_w)
    }

    /// Open a pane from the Start screen or the tab strip.
    ///
    /// Files is the pane that has to work now; the other two take the tab and
    /// say they are not built, which is what tells a reader pressing Terminal
    /// that they did press it.
    pub(crate) fn open_right_tab(
        &mut self,
        tab: RightTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.right.open = true;
        self.right.tab = Some(tab);
        if !self.right.open_panes.contains(&tab) {
            self.right.open_panes.push(tab);
        }
        self.right.catalog_open = false;
        self.record_panel_recent(tab);
        if tab == RightTab::Tasks {
            // One store read per open, never per frame. Peer delegations are
            // read from the same reload the federation pane uses, so the tree
            // can distinguish a peer from a local child without inventing one.
            *self.right.tasks.borrow_mut() = None;
            self.reload_federation_quiet();
        }
        if tab == RightTab::Federation {
            self.reload_federation();
        }
        if tab == RightTab::Terminal {
            if self.right.terminal.is_none() {
                let cwd = self.state.workspace_root().unwrap_or_default();
                // The SAME spec the shell tool runs under, derived from the
                // live policy and the effective permission. A native libghostty
                // surface owns its own PTY, so the sandbox cannot join the
                // shell: the model records that answer and the status line says
                // it.
                let spec = self.state.sandbox_spec("terminal");
                // The policy that released it, in the same precedence the build
                // applies: the settings master switch wins over a preset's arm,
                // and a permission release sets neither. The sentence is passed
                // BEFORE the spawn, because it overrides BSH_SANDBOX for the
                // shell itself and not only the status line.
                let sandbox = self.state.effective().sandbox;
                let off = if sandbox.off_by_settings {
                    Some((
                        crate::terminal::SandboxOff::Settings,
                        self.state.lang.tr("right.terminal.sandboxOffSettings"),
                    ))
                } else if sandbox.off_by_preset {
                    Some((
                        crate::terminal::SandboxOff::Preset,
                        self.state.lang.tr("right.terminal.sandboxOff"),
                    ))
                } else {
                    None
                };
                self.right.terminal = Some(match off {
                    Some(off) => crate::terminal::Terminal::new_with_sandbox_note(
                        cwd,
                        self.state.lang,
                        spec.as_deref(),
                        Some(off),
                    ),
                    None => crate::terminal::Terminal::new_with_sandbox(
                        cwd,
                        self.state.lang,
                        spec.as_deref(),
                    ),
                });
            }
            self.ensure_terminal_view(window, cx);
        } else {
            self.hide_terminal_view(cx);
        }
        if tab == RightTab::Files && self.files.roots.is_empty() && self.files.tabs.is_empty() {
            self.refresh_files(cx);
        }
        if tab == RightTab::Browser {
            self.browser_prewarm(cx);
        }
        self.sync_terminal(cx);
        cx.notify();
    }

    /// Navigate from the address bar: the one control a reader presses to open
    /// a page.
    pub(crate) fn browser_submit(&mut self, cx: &mut Context<Self>) {
        let typed = self.browser_url.read(cx).value().to_string();
        let url = normalize_address(&typed);
        if url.is_empty() {
            cx.notify();
            return;
        }
        self.browser_go(url, cx);
    }

    /// Start a navigation, showing the loading state on this frame.
    pub(crate) fn browser_go(&mut self, url: String, cx: &mut Context<Self>) {
        let handle = self.state.browser.clone();
        // Begin here, not on the worker: the press has to have an answer before
        // the request exists, or the pane looks inert for a frame or two.
        let seq = handle.mutate(|browser| browser.begin(&url));
        // The capture on screen pictures the page this navigation replaces, so
        // it leaves with it. Otherwise a pre-warm's `about:blank` whiteness, or
        // the previous page, stands in for the one being loaded. The tile is
        // released by the next render, the same way a replaced frame is.
        if let Some(previous) = self.browser_render.take() {
            self.browser_render_stale.push(previous);
        }
        self.browser_refresh_pane();
        cx.notify();
        // The FIRST navigation from the address bar has no view yet: the render
        // builds one from the address this call just set. Spawning the
        // text/CDP load here would start Chromium for a page the view is about
        // to render, and where Chromium cannot start it replaced the page with
        // the text view - which is what made the first Enter look inert.
        if self.browser_webview.is_none() && !cfg!(test) {
            return;
        }
        // The native view is the page: load it there and stop. The HTTP fetch
        // below feeds the TEXT model the agent tools read; the reader's pane
        // does not need it, and its failures would draw a banner over a page
        // that actually loaded.
        if let Some(webview) = self.browser_webview.clone() {
            if url.starts_with("http://") || url.starts_with("https://") {
                webview.update(cx, |view, _| view.load_url(&url));
                cx.notify();
                return;
            }
        }
        // The GPUI background executor, not the runner's tokio runtime: this is
        // work the window started, and it is the executor the test scheduler
        // can drive deterministically.
        let task = cx.background_executor().spawn({
            let handle = handle.clone();
            let url = url.clone();
            // The pane reads the frame, which is live while the tool path's
            // idle window would run: one paint beat after the load event, and
            // the toolbar, address bar and history buttons keep up with what
            // is already on screen.
            async move {
                crate::browser::load(&handle, &url, seq, crate::browser::engine::Settle::Paint)
            }
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |this, cx| {
                this.browser_refresh_pane();
                // A refused or failed load already carries its sentence in the
                // pane; the notice ring would only repeat it.
                if let Err(failure) = &outcome {
                    if failure.kind == "scheme" {
                        this.state.notice(failure.tool_message());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Build the native browser view, once the Browser tab has a live window.
    ///
    /// The view needs a real window and a native child view, so it is built
    /// from a deferred task rather than during paint. Headless tests keep the
    /// text/capture path: a native view cannot be exercised there, and the
    /// suite would be claiming coverage it does not have.
    pub(crate) fn browser_ensure_webview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if cfg!(test) || self.browser_webview.is_some() || self.browser_webview_pending {
            return;
        }
        if !self.right.open || !matches!(self.right.tab, Some(RightTab::Browser)) {
            return;
        }
        // Nothing to show yet: a native view on `about:blank` would cover the
        // pane's empty state (the globe and prompt) with a white rectangle.
        // The view is built by the first navigation, which sets the address.
        if self
            .state
            .browser
            .read(|browser| browser.address().is_empty())
        {
            return;
        }
        self.browser_webview_pending = true;
        let shell = cx.entity();
        window.defer(cx, move |window, cx| {
            shell.update(cx, |this, cx| this.build_browser_webview(window, cx));
        });
    }

    /// Attach the native view and point it at what the pane is showing.
    fn build_browser_webview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.browser_webview_pending = false;
        if self.browser_webview.is_some() {
            return;
        }
        // Every navigation reports its URL here, including a link click, a
        // redirect and a history move, so the address bar follows the page
        // without asking the view for a URL it cannot always give.
        let nav = self.browser_script.clone();
        let popups = self.browser_script.clone();
        // A real browser UA: Wry's default is a sparse `AppleWebKit/605.1.15`
        // with no Safari token, and sites that sniff it serve a stub (MEASURED:
        // baidu.com answered with a document that had no body at all).
        let user_agent = if cfg!(target_os = "macos") {
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15"
        } else if cfg!(target_os = "windows") {
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36"
        } else {
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36"
        };
        let builder = wry::WebViewBuilder::new()
            .with_user_agent(user_agent)
            .with_navigation_handler(move |url: String| {
                nav.note_navigation(url);
                true
            })
            // A link with `target="_blank"` (or `window.open`) asks for a NEW
            // window. The pane has one page, so the request is denied and the
            // URL is loaded here instead - otherwise the click did nothing.
            .with_new_window_req_handler(move |url: String, _features| {
                popups.note_new_window(url);
                wry::NewWindowResponse::Deny
            });
        match gpui_webview::WebView::build(builder, window, cx) {
            Ok(view) => {
                let entity = cx.new(|_| view);
                // A pane opened after a navigation shows the page already
                // asked for, not a blank view.
                let address = self
                    .state
                    .browser
                    .read(|browser| browser.address().to_string());
                if !address.is_empty() {
                    entity.update(cx, |view, _| view.load_url(&address));
                }
                // From here the agent tools drive THIS page instead of
                // starting a second browser.
                self.state.browser.set_script(self.browser_script.clone());

                self.browser_webview = Some(entity);
            }
            Err(error) => {
                eprintln!("bsh: the browser view could not be created: {error}");
                self.state
                    .notice(format!("the browser view could not be created: {error}"));
            }
        }
        cx.notify();
    }

    /// Keep the native view sized, visible, and following the page's own URL.
    fn sync_browser_webview(&mut self, cx: &mut Context<Self>) {
        let Some(webview) = self.browser_webview.clone() else {
            return;
        };
        // The agent tools' queued work runs HERE, because the view is
        // UI-thread-only: a tool posted the command and is waiting on its reply.
        for job in self.browser_script.drain() {
            use crate::browser::script::ScriptCommand;
            match job.command {
                ScriptCommand::Eval(js) => {
                    let reply = job.reply;
                    let _ = webview
                        .read(cx)
                        .evaluate_script_with_callback(&js, move |out| {
                            let _ = reply.send(Ok(out));
                        });
                }
                ScriptCommand::Load(url) => {
                    webview.update(cx, |view, _| view.load_url(&url));
                    let _ = job.reply.send(Ok(String::new()));
                }
            }
        }
        let visible = self.right.open
            && matches!(self.right.tab, Some(RightTab::Browser))
            // A native child view is drawn ABOVE the GPUI tree on this
            // backend, so an open dialog or the tab catalogue would be hidden
            // behind the page.
            && !self.right.catalog_open
            && matches!(self.state.overlay, crate::state::Overlay::None);
        if let Some(bounds) = self.browser_bounds.get() {
            let rect = wry::Rect {
                position: wry::dpi::Position::Logical(wry::dpi::LogicalPosition::new(
                    f64::from(bounds.origin.x.as_f32()),
                    f64::from(bounds.origin.y.as_f32()),
                )),
                size: wry::dpi::Size::Logical(wry::dpi::LogicalSize::new(
                    f64::from(bounds.size.width.as_f32().max(0.0)),
                    f64::from(bounds.size.height.as_f32().max(0.0)),
                )),
            };
            let _ = webview.read(cx).set_bounds(rect);
        }
        let _ = webview.read(cx).set_visible(visible);
        // A link that asked for a new window: the pane has one page, so load
        // it here. This is what makes a search RESULT open at all.
        if let Some(url) = self.browser_script.take_new_window() {
            if url.starts_with("http://") || url.starts_with("https://") {
                webview.update(cx, |view, _| view.load_url(&url));
                self.state.browser.mutate(|browser| browser.follow(&url));
                self.browser_refresh_pane();
                cx.notify();
            }
        }
        // Follow a link click or a redirect from the navigation handler. This
        // never calls `WebView::url()`: that unwraps a nil NSURL before the
        // first commit, which aborts the process.
        let navigated = self.browser_script.take_navigation();
        if let Some(url) = navigated {
            if !url.is_empty() && url != "about:blank" {
                let changed = self.state.browser.read(|browser| browser.address() != url);
                if changed {
                    self.state.browser.mutate(|browser| browser.follow(&url));
                    self.browser_refresh_pane();
                    cx.notify();
                }
            }
        }
    }

    /// Start the rendered engine when the Browser tab is first shown.
    ///
    /// MEASURED: launching Chromium (process + CDP handshake) takes ~1.3 s in
    /// a debug build here. A pre-warm
    /// moves that cost to the moment the pane appears, so the FIRST navigation
    /// does not pay it. The engine is left with no capture pump (the pump
    /// starts on the first painted frame), so a pane that is shown and never
    /// used costs an idle process and nothing else; closing the pane stops it.
    pub(crate) fn browser_prewarm(&mut self, cx: &mut Context<Self>) {
        // The pane's backend is the native view, built from a frame with a live
        // window (`browser_ensure_webview`). Nothing is launched here: the CDP
        // engine remains for the agent tools, which drive a page
        // programmatically and need the protocol.
        //
        // The headless suite still runs on the capture path, so there it still
        // wants the engine warm.
        if !cfg!(test) {
            return;
        }
        if self.state.browser.engine_ready() || self.state.browser.engine_error().is_some() {
            return;
        }
        let handle = self.state.browser.clone();
        let task = cx
            .background_executor()
            .spawn(async move { handle.ensure_engine().map(|_| ()) });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |this, cx| {
                this.browser_refresh_pane();
                let _ = outcome;
                cx.notify();
            });
        })
        .detach();
    }

    /// Go back, forward or reload through the pane's own controls.
    ///
    /// Back and forward restore a cached document, so they ask no policy
    /// question and make no request; reload is a new navigation and goes
    /// through the same gate a fresh open does.
    pub(crate) fn browser_history(&mut self, step: isize, cx: &mut Context<Self>) {
        if let Some(webview) = self.browser_webview.clone() {
            if step < 0 {
                let _ = webview.update(cx, |view, _| view.back());
            } else {
                let _ = webview.update(cx, |view, _| view.forward());
            }
            cx.notify();
            return;
        }
        let outcome = self.state.browser.mutate(|browser| {
            if step < 0 {
                browser.back()
            } else {
                browser.forward()
            }
        });
        self.browser_refresh_pane();
        if let Err(message) = outcome {
            self.state.notice(message);
        } else if let Some(previous) = self.browser_render.take() {
            // The restored document is a different page, so the capture on
            // screen is not it; the pump repaints from the cached document.
            self.browser_render_stale.push(previous);
        }
        cx.notify();
    }

    /// Reload the page on screen.
    pub(crate) fn browser_reload(&mut self, cx: &mut Context<Self>) {
        if let Some(webview) = self.browser_webview.clone() {
            let _ = webview.read(cx).reload();
            cx.notify();
            return;
        }
        let url = self
            .state
            .browser
            .read(|browser| browser.document().map(|doc| doc.url.clone()));
        match url {
            Some(url) => self.browser_go(url, cx),
            None => cx.notify(),
        }
    }

    /// Fill the address bar from the browser when a navigation moved it.
    ///
    /// Only a real change retriggers it: set_value always notifies, and a
    /// per-frame write would both repaint the field forever and fight a reader
    /// who is typing. The guard is the URL that was last written, not the
    /// field's focus.
    pub(crate) fn browser_sync_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.browser_pane.as_ref() else {
            return;
        };
        let url = view.url.clone();
        if url.is_empty() || url == self.browser_shown_url {
            return;
        }
        self.browser_shown_url.clone_from(&url);
        self.browser_url
            .update(cx, |state, cx| state.set_value(url, window, cx));
    }

    /// Rebuild the pane's cached view when the browser moved.
    ///
    /// Called from render, so it only costs an atomic read on a frame that did
    /// not change the browser; the clone of a page's text happens once per
    /// change, not once per frame.
    /// The Browser pane is going away: stop the engine and forget the frame.
    ///
    /// A tab SWITCH only hides the pane (the pump stops, the process stays);
    /// closing the tab is the reader saying the page is done, so the process
    /// goes with it rather than idling behind a pane nobody can see.
    pub(crate) fn browser_pane_closed(&mut self) {
        self.right.tab = None;
        self.state.browser.stop_engine();
        // The frame on screen still owns an atlas tile until a render drops it,
        // so it is queued rather than forgotten.
        if let Some(previous) = self.browser_render.take() {
            self.browser_render_stale.push(previous);
        }
        self.browser_frame_seq = 0;
        // A pending resize belongs to the engine that is gone; a new engine
        // starts at its own default viewport.
        self.browser_resize_want = None;
        self.browser_resize_at = None;
        self.browser_bounds.set(None);
        // A gesture belongs to the page that was on screen; closing it must
        // not leave the pane thinking a press is still down.
        self.browser_pressed = None;
        self.browser_click = None;
    }

    pub(crate) fn browser_refresh_pane(&mut self) {
        let revision = self.state.browser.revision();
        if self.browser_pane.is_some() && revision == self.browser_rev {
            return;
        }
        let view = self.state.browser.read(|browser| browser.pane_view());
        self.browser_pane = Some(view);
        self.browser_rev = revision;
    }

    /// Keep the rendered page's viewport, capture rate and frame tied to what
    /// is on screen.
    ///
    /// This runs on the window's own clock, not per frame: a capture is a
    /// PNG decode on the engine's pump thread, and the pane only builds a
    /// @@RenderImage@@ from bytes that already exist. A hidden tab stops the
    /// pump entirely, so a page nobody is looking at costs nothing.
    pub(crate) fn sync_browser(&mut self, cx: &mut Context<Self>) {
        // The native view first: it is the page the reader sees, and its size,
        // visibility and URL are what the rest of this clock is about.
        self.sync_browser_webview(cx);
        // With no view, a queued tool command must hear that rather than wait
        // out its timeout.
        if self.browser_webview.is_none() {
            for job in self.browser_script.drain() {
                crate::browser::script::reject(job, "no browser view is open in the pane");
            }
        }
        let browser = self.state.browser.clone();
        let live = browser.engine_ready();
        let visible = self.right.open && matches!(self.right.tab, Some(RightTab::Browser));
        let mut dirty = false;
        // The pane's cached view first: the pump's answer below depends on
        // whether there is a page to capture, and the revision is the only
        // thing that says the view moved.
        let revision = browser.revision();
        if self.browser_pane.is_none() || revision != self.browser_rev {
            let view = browser.read(|browser| browser.pane_view());
            self.browser_pane = Some(view);
            self.browser_rev = revision;
            dirty = true;
        }
        if live {
            if let Some(engine) = browser.engine() {
                // A pane with no document captures nothing. The pre-warm's
                // `about:blank` would otherwise be probed and PNG-decoded eight
                // times a second and published as a full-viewport texture this
                // pane does not draw, which is work nobody asked for.
                let has_page = self
                    .browser_pane
                    .as_ref()
                    .is_some_and(|view| frame_is_the_page(view.status));
                // Capture only a SETTLED page. MEASURED: a capture queued while
                // a page is still loading waits for the renderer for 0.6-1.2 s
                // and returns a half-painted frame, while the same capture on a
                // settled page costs 30-120 ms. The last frame stays on screen
                // for the length of the load, which is what a reader expects.
                let settled = self
                    .browser_pane
                    .as_ref()
                    .is_some_and(|view| matches!(view.status, crate::browser::Status::Ready));
                engine.set_streaming(visible && has_page && settled);
                if visible {
                    // The viewport follows the geometry the frame is actually
                    // drawn in, so the page reflows instead of being upscaled.
                    // A resize is a CDP round trip, so it must not run on the
                    // render path: the want is recorded here and applied off
                    // the UI thread, at most one override per tick window, so
                    // a pane drag reflows at a bounded rate and a settled pane
                    // still catches the final size. The pump's probe publishes
                    // the reflow on its next capture, so no dirty flag is
                    // needed here.
                    if let Some(bounds) = self.browser_bounds.get() {
                        let width = bounds.size.width.as_f32().round().max(0.0) as u32;
                        let height = bounds.size.height.as_f32().round().max(0.0) as u32;
                        let scale = self.browser_scale.get();
                        if width >= 320
                            && height >= 240
                            && (engine.viewport() != (width, height) || engine.scale() != scale)
                        {
                            self.browser_resize_want = Some((width, height, scale));
                        }
                    }
                    if let Some(want) = self.browser_resize_want {
                        let due = self
                            .browser_resize_at
                            .is_none_or(|at| at.elapsed() >= BROWSER_RESIZE_EVERY);
                        if due {
                            self.browser_resize_want = None;
                            self.browser_resize_at = Some(std::time::Instant::now());
                            let engine = engine.clone();
                            cx.background_executor()
                                .spawn(async move {
                                    let _ = engine.resize((want.0, want.1), want.2);
                                })
                                .detach();
                        }
                    }
                }
            }
        } else if let Some(previous) = self.browser_render.take() {
            // The engine is gone, so the last frame leaves the screen - and its
            // atlas tile is released the same way a replaced one is.
            self.browser_render_stale.push(previous);
            dirty = true;
        }
        if let Some((seq, frame)) = browser.frames().take_newer(self.browser_frame_seq) {
            self.browser_frame_seq = seq;
            let size = (frame.width, frame.height);
            if let Some(render) = render_frame(frame) {
                self.browser_frame_size = size;
                // A new frame is a new `RenderImage::id`, so the atlas
                // caches its tile separately from the one it replaces. The
                // replaced frame is queued for release on the next render.
                if let Some(previous) = self.browser_render.replace(std::sync::Arc::new(render)) {
                    self.browser_render_stale.push(previous);
                }
                dirty = true;
            }
        }
        if dirty {
            cx.notify();
        }
    }

    /// The live page inside the pane: the frame, its pointer, its wheel and its
    /// keys, or the named reason the text view is what is on screen.
    fn browser_live_body(&self, cx: &mut Context<Self>) -> AnyElement {
        // The native view owns the page: WKWebView/WebView2/WebKitGTK
        // composites it, and mouse, keyboard, IME, scrolling, selection and
        // video are the platform's own. Nothing is captured and nothing is
        // forwarded, so the pane behaves as the engine does by definition.
        if let Some(webview) = self.browser_webview.clone() {
            return div()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(webview)
                .into_any_element();
        }
        let view = self.browser_pane.clone().unwrap_or_default();
        // A capture is only the reader's page when the browser holds a
        // document for it. The engine's pre-warm paints `about:blank`, so a
        // frame with no document behind it is NOT a page: drawing it leaves a
        // plain white rectangle with no globe, no prompt and no way to start -
        // the blank pane this rule exists to prevent. `browser_go` releases
        // the frame it replaces for the same reason, so a stale capture cannot
        // stand in for the page being loaded either.
        let Some(render) = self
            .browser_render
            .clone()
            .filter(|_| frame_is_the_page(view.status))
        else {
            // Never a blank rectangle: the text view states what the pane is
            // doing (nothing yet, loading this URL, or why a load failed) and
            // the engine's own sentence sits above it when it could not start.
            return div()
                .flex_1()
                .min_h_0()
                .w_full()
                .flex()
                .flex_col()
                .child(self.browser_engine_notice())
                .child(self.browser_body(view, cx))
                .into_any_element();
        };
        let bounds = self.browser_bounds.clone();
        let tracked = bounds.clone();
        // Non-printable keys (Backspace, the arrows, Delete) reach AppKit's input
        // context BEFORE gpui's key listeners, and a view with no text input
        // handler never gets them back: MEASURED, Backspace was swallowed and a
        // field could not be edited. This canvas installs a handler so those
        // commands are routed back to the key listener. It does not type: a
        // printable key's character is already sent by the listener as a real
        // keydown plus char, and inserting it here too would double it.
        let command_focus = self.browser_focus.clone();
        let command_shell = cx.entity();
        // The window's device scale factor, read at prepaint and carried to the
        // engine's next resize: a capture at the screen's own scale is what
        // draws one texture pixel per screen pixel, which is the difference
        // between a sharp page and a blurry one.
        let scale = self.browser_scale.clone();
        let mut column = div().flex_1().min_h_0().w_full().flex().flex_col();
        // A failed navigation keeps the page the reader was already reading
        // and says why above it, which is the rule the text view follows.
        // Without this the live pane swallowed every failure: the frame stayed
        // and the sentence the pane owed the reader was never drawn.
        if let Some(failure) = view.failure.clone() {
            column = column.child(self.browser_failure(failure));
        }
        column
            .child(
                div()
                    .on_children_prepainted(move |children, window, _| {
                        tracked.set(children.first().copied());
                        scale.set(window.scale_factor());
                    })
                    .id("browser-live")
                    .debug_selector(|| String::from("browser-render-frame"))
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .relative()
                    .overflow_hidden()
                    .track_focus(&self.browser_focus)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            // A press enters the page: keys go there from now on.
                            window.focus(&this.browser_focus, cx);
                            this.browser_pointer_down(
                                event.position,
                                event.button,
                                event.click_count,
                                event.modifiers,
                            );
                            cx.notify();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            // A right press enters the page too: the focus
                            // follows the gesture whichever button made it.
                            window.focus(&this.browser_focus, cx);
                            this.browser_pointer_down(
                                event.position,
                                event.button,
                                event.click_count,
                                event.modifiers,
                            );
                            cx.notify();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            window.focus(&this.browser_focus, cx);
                            this.browser_pointer_down(
                                event.position,
                                event.button,
                                event.click_count,
                                event.modifiers,
                            );
                            cx.notify();
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                        // Hover updates :hover; a held button turns the same
                        // event into a drag. Both reach the page now, so a
                        // reader can hover a link and select text.
                        this.browser_pointer_move(
                            event.position,
                            event.pressed_button,
                            event.modifiers,
                        );
                        cx.notify();
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseUpEvent, _, cx| {
                            this.browser_pointer_up(event.position, event.button, event.modifiers);
                            cx.notify();
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Right,
                        cx.listener(|this, event: &MouseUpEvent, _, cx| {
                            this.browser_pointer_up(event.position, event.button, event.modifiers);
                            cx.notify();
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Middle,
                        cx.listener(|this, event: &MouseUpEvent, _, cx| {
                            this.browser_pointer_up(event.position, event.button, event.modifiers);
                            cx.notify();
                        }),
                    )
                    .on_scroll(cx.listener(|this, event: &ScrollEvent, window, cx| {
                        this.browser_scroll(event, window.line_height());
                        cx.notify();
                    }))
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        this.browser_key(event, cx);
                        cx.notify();
                    }))
                    .child(
                        img(ImageSource::Render(render))
                            .w(relative(1.0))
                            .h(relative(1.0)),
                    )
                    .child(
                        canvas(
                            |_bounds, _window, _cx| (),
                            move |bounds, _, window, cx| {
                                window.handle_input(
                                    &command_focus,
                                    gpui::ElementInputHandler::new(bounds, command_shell.clone()),
                                    cx,
                                );
                            },
                        )
                        .absolute()
                        .top(px(0.0))
                        .left(px(0.0))
                        .size_full(),
                    ),
            )
            .into_any_element()
    }

    /// The engine's own sentence when the text view is what is drawn.
    ///
    /// A blank rectangle is not an acceptable state: if the rendered engine
    /// could not start, the pane says why in the engine's own words, with the
    /// install hint, above the text view.
    fn browser_engine_notice(&self) -> AnyElement {
        let Some(error) = self.state.browser.engine_error() else {
            return div().into_any_element();
        };
        let selector = format!("browser-engine-{}", error.kind());
        div()
            .debug_selector(move || selector.clone())
            .flex_none()
            .w_full()
            .px(px(theme::RIGHT_PAD))
            .py(px(8.0))
            .bg(rgb(theme::hover()))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(txt(error.message(), theme::CHIP_SIZE, theme::error()).line_height(px(16.0)))
            .child(txt(error.hint(), theme::CHIP_SIZE, theme::muted()).line_height(px(15.0)))
            .into_any_element()
    }

    /// Map a window point inside the frame to PAGE coordinates.
    ///
    /// The frame is drawn scaled to the pane's geometry, so the point a reader
    /// pressed and the point the page receives are related by the viewport
    /// ratio. None means the press was outside the frame (a clipped edge).
    fn browser_page_point(&self, position: Point<Pixels>) -> Option<(f32, f32)> {
        let bounds = self.browser_bounds.get()?;
        let engine = self.state.browser.engine()?;
        page_point(bounds, engine.viewport(), position, false)
    }

    /// Map a window point to the page, CLAMPED into the frame.
    ///
    /// This is the captured-pointer mapping: a move or release that leaves the
    /// pane still belongs to the gesture, at the page's edge.
    fn browser_page_point_clamped(&self, position: Point<Pixels>) -> Option<(f32, f32)> {
        let bounds = self.browser_bounds.get()?;
        let engine = self.state.browser.engine()?;
        page_point(bounds, engine.viewport(), position, true)
    }

    /// A press inside the page: focus the pane, then HOLD that button down.
    ///
    /// The count is the platform's and the release reuses it, so the page's
    /// `click`/`dblclick` (and their `detail`) see one number. The button is
    /// forwarded as it arrived: a right press is the page's `contextmenu`, a
    /// middle press its `auxclick`.
    fn browser_pointer_down(
        &mut self,
        position: Point<Pixels>,
        button: MouseButton,
        click_count: usize,
        modifiers: Modifiers,
    ) {
        let Some(pointer) = pointer_button(button) else {
            return;
        };
        // One gesture at a time. A second button pressed while one is held is
        // not an event the page can express, and treating it as a new press
        // would break the drag or selection the reader is still making.
        if self.browser_pressed.is_some() {
            return;
        }
        let Some((x, y)) = self.browser_page_point(position) else {
            return;
        };
        let click = BrowserClick::press(self.browser_click.as_ref(), button, click_count);
        let count = click.count();
        self.browser_click = Some(click);
        self.browser_modifiers = modifiers;
        if let Some(engine) = self.state.browser.engine() {
            // Only a press the page actually received is a held gesture; a pane
            // with no engine has nothing to release later.
            self.browser_pressed = Some(button);
            // The held chord rides the press: a page reads `shiftKey` here, so
            // Shift+click extends a selection instead of replacing it.
            let _ =
                engine.pointer_down_with(x, y, count, pointer, chord_modifiers(&modifiers), true);
        }
    }

    /// A move: hover when no button is held, a drag while one is.
    ///
    /// Hover moves are throttled to one forwarded event per ~16 ms. A forward
    /// is fire-and-forget (no reply is waited on), but each one is still a
    /// queued CDP command, and a page only needs enough of them to update
    /// :hover; a drag is never throttled, because every point is part of the
    /// selection.
    ///
    /// The held button rides the move, which is the whole selection mechanism:
    /// the page runs its own default (the dragged range, the word under a
    /// double click, a textarea's own selection) as soon as it sees a move with
    /// that button down. The pane used to synthesize the selection from carets
    /// instead, which failed wherever an endpoint was not text - a margin, a
    /// gap between blocks, a field - and could not select a field's text at all.
    fn browser_pointer_move(
        &mut self,
        position: Point<Pixels>,
        pressed: Option<MouseButton>,
        modifiers: Modifiers,
    ) {
        let dragging = pressed.is_some() && pressed == self.browser_pressed;
        let now = std::time::Instant::now();
        if !dragging {
            if let Some(last) = self.browser_last_move {
                if now.duration_since(last) < std::time::Duration::from_millis(16) {
                    return;
                }
            }
        }
        self.browser_last_move = Some(now);
        let Some((x, y)) = self.browser_page_point(position) else {
            return;
        };
        if let Some(engine) = self.state.browser.engine() {
            // Shift+drag EXTENDS the selection; without the chord the page
            // replaces it, which is not what the reader held Shift for.
            let _ = engine.pointer_move_with(
                x,
                y,
                self.browser_pressed.and_then(pointer_button),
                chord_modifiers(&modifiers),
            );
        }
    }

    /// A drag move that left the frame.
    ///
    /// The frame's own move handler stops firing at its edge (gpui routes a
    /// move by hit test), so the window's move handler calls this with the
    /// clamped edge point while the press is held. A move that is still inside
    /// the frame is left to the frame, so a drag point is never sent twice.
    pub(crate) fn browser_drag_outside(&mut self, position: Point<Pixels>) {
        if self.browser_pressed.is_none() || self.browser_page_point(position).is_some() {
            return;
        }
        let Some((x, y)) = self.browser_page_point_clamped(position) else {
            return;
        };
        if let Some(engine) = self.state.browser.engine() {
            let _ = engine.pointer_move_with(
                x,
                y,
                self.browser_pressed.and_then(pointer_button),
                chord_modifiers(&self.browser_modifiers),
            );
        }
    }

    /// The release that completes a press of `button`.
    ///
    /// A release of any OTHER button is not this gesture's end: releasing the
    /// right button while the left is held leaves the left press down.
    fn browser_pointer_up(
        &mut self,
        position: Point<Pixels>,
        button: MouseButton,
        modifiers: Modifiers,
    ) {
        if self.browser_pressed != Some(button) {
            return;
        }
        let Some(pointer) = pointer_button(button) else {
            return;
        };
        self.browser_pressed = None;
        // The stored count is the one the press carried, so both halves of the
        // gesture name the same click to the page.
        let count = self.browser_click.map_or(1, |click| click.count());
        let Some((x, y)) = self.browser_page_point_clamped(position) else {
            return;
        };
        if let Some(engine) = self.state.browser.engine() {
            let _ = engine.pointer_up_with(x, y, count, pointer, chord_modifiers(&modifiers), true);
        }
    }

    /// The release that completed a press but landed outside the frame.
    ///
    /// This is the captured-pointer half of the gesture: the page gets a last
    /// drag move and a real `mouseReleased` at the frame's clamped edge, so its
    /// selection and its drag state settle instead of staying pressed forever.
    /// Called from the window's own mouse-up listeners; a release inside the
    /// frame was already handled by `browser_pointer_up`, which clears the
    /// pressed flag first.
    pub(crate) fn browser_release_outside(&mut self, position: Point<Pixels>, button: MouseButton) {
        if self.browser_pressed != Some(button) {
            return;
        }
        let Some(pointer) = pointer_button(button) else {
            return;
        };
        self.browser_pressed = None;
        let count = self.browser_click.map_or(1, |click| click.count());
        let Some((x, y)) = self.browser_page_point_clamped(position) else {
            return;
        };
        if let Some(engine) = self.state.browser.engine() {
            let modifiers = chord_modifiers(&self.browser_modifiers);
            let _ = engine.pointer_move_with(x, y, Some(pointer), modifiers);
            let _ = engine.pointer_up_with(x, y, count, pointer, modifiers, true);
        }
    }

    /// Forward a wheel event.
    ///
    /// The page scrolls itself: the pane only maps the point and the deltas, so
    /// a page's own wheel handler, its smooth scrolling and its nested scrollers
    /// all behave as they do in a browser. [`page_wheel`] owns the sign
    /// conversion and the clamp.
    fn browser_scroll(&mut self, event: &ScrollEvent, line_height: Pixels) {
        let Some(bounds) = self.browser_bounds.get() else {
            return;
        };
        let Some(engine) = self.state.browser.engine() else {
            return;
        };
        let Some((x, y, delta_x, delta_y)) = page_wheel(
            bounds,
            engine.viewport(),
            event.position,
            &event.delta,
            line_height,
        ) else {
            return;
        };
        // The chord rides the wheel: Ctrl/Cmd+wheel is the zoom a map or an
        // editor implements itself, Shift+wheel a horizontal scroll.
        let _ = engine.scroll_with(
            x,
            y,
            delta_x,
            delta_y,
            chord_modifiers(&event.modifiers),
            true,
        );
    }

    /// Forward a key: printable text as text, the named keys as keys.
    fn browser_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(engine) = self.state.browser.engine() else {
            return;
        };
        let keystroke = &event.keystroke;
        if browser_shortcut(keystroke, &engine, cx) {
            return;
        }
        if let Some(named) = named_key(&keystroke.key) {
            // The modifiers ride along: Shift+Tab walks backwards, Shift+Arrow
            // extends a selection and Cmd/Ctrl+Arrow jumps a word, which all
            // arrive as the bare key otherwise.
            let _ = engine.press_with(named, chord_modifiers(&keystroke.modifiers), true);
            return;
        }
        let Some(text) = typed_text(keystroke) else {
            return;
        };
        if text.chars().count() == 1 {
            // One printable character: a real browser's keydown plus char, so
            // a page listening for keydown sees the key AND the field receives
            // exactly that character once.
            let _ = engine.type_key(text, chord_modifiers(&keystroke.modifiers), true);
            self.browser_typed = Some((std::time::Instant::now(), text.to_string()));
        } else {
            // A composition (CJK/IME) or an input source's multi-character
            // commit: insert it as text, the route that does not pretend to be
            // a key this keyboard does not have.
            //
            // Recorded like a typed key, because the platform can deliver the
            // same commit to the input handler as well: MEASURED, `你好` landed
            // in the field twice before this.
            let _ = engine.insert_text(text, true);
            self.browser_typed = Some((std::time::Instant::now(), text.to_string()));
        }
    }

    /// Re-read the workspace policy when the file on disk changed.
    ///
    /// Rides the terminal clock, which is one stat per tick and no work at all
    /// when the stamp is unchanged: a policy edit takes effect without a
    /// restart, and a broken edit keeps the last good policy instead of
    /// wedging the pane.
    pub(crate) fn refresh_terminal_policy(&mut self, cx: &mut Context<Self>) {
        let refreshed = match self.right.terminal.as_mut() {
            Some(terminal) => terminal.refresh_policy(),
            None => false,
        };
        if refreshed {
            cx.notify();
        }
    }

    /// Keep the pane's surface in step with the window.
    ///
    /// Both jobs ride the terminal clock rather than the paint phase: the
    /// palette and the shell's liveness are state, and the window must not be
    /// mutated while it is painting. Ghostty measures its own grid from the
    /// frame the pane's canvas hands it, so a panel resize needs nothing here.
    pub(crate) fn sync_terminal(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.right.terminal_view.clone() else {
            return;
        };
        // The palette follows the window's theme; applying it is a compare and,
        // only on a real change, one configuration reload.
        let theme = super::terminal_view::terminal_theme();
        let font_size =
            crate::theme::terminal_font_size(self.state.draft().global.general.content_font_size);
        if view.update(cx, |view, _| view.sync_theme(theme, font_size)) {
            cx.notify();
        }
        // A shell that exited is noticed through Ghostty's own answer; the
        // surface keeps the last frame, so the status line is what says the
        // shell is gone.
        let alive = view.read(cx).is_alive();
        if alive != self.terminal_running {
            self.terminal_running = alive;
            cx.notify();
        }
    }

    /// The panel resize strip's press: the same origin/base dance the sidebar
    /// strip does, so both handles behave identically under the hand.
    pub(crate) fn begin_right_panel_drag(&mut self, pointer_x: f32, _cx: &mut Context<Self>) {
        self.right.drag = Some(SideDrag {
            origin_x: pointer_x,
            base: self.right.user_w.unwrap_or_else(|| self.right_panel_w()),
            moved: false,
        });
    }

    /// Track the strip's move, clamped to the panel's own contract range.
    pub(crate) fn drag_right_panel_width(&mut self, pointer_x: f32, cx: &mut Context<Self>) {
        let Some(drag) = self.right.drag.as_mut() else {
            return;
        };
        let delta = pointer_x - drag.origin_x;
        if delta.abs() > 2.0 {
            drag.moved = true;
        }
        // The strip sits on the panel's left edge, so dragging left widens it.
        if !drag.moved {
            return;
        }
        // Resizing by hand is the reader asking for a width, so it also gives up
        // the maximised column rather than fighting it.
        self.right.maximized = false;
        let width = (drag.base - delta)
            .round()
            .clamp(theme::RIGHT_PANEL_MIN, theme::RIGHT_PANEL_MAX);
        let width = Some(width);
        // The width is read into the column's layout this frame, so a move must
        // repaint: the sidebar's own drag notifies for exactly this reason, and
        // without it the panel only followed the pointer when something else
        // happened to schedule a frame.
        if self.right.user_w != width {
            self.right.user_w = width;
            cx.notify();
        }
    }

    /// Release the strip: the width stands, unless it was a click.
    pub(crate) fn commit_right_panel_width(&mut self, _pointer_x: f32, cx: &mut Context<Self>) {
        // A press that never moved is a click on the seam, not a resize: it must
        // leave the width the reader dragged out alone. It used to clear the
        // preference, so touching the handle threw the width away - the
        // sidebar's own seam has always kept it.
        self.right.drag = None;
        cx.notify();
    }

    /// Drop the file pane's view of the workspace it was showing.
    ///
    /// Every path in the pane is relative, so a switch changes what each one
    /// MEANS: an open tab is a buffer whose rel would now resolve into another
    /// workspace's file, and saving it would write there. So the tabs go, the
    /// cached tree goes with them, and the pane re-lists whatever workspace is
    /// active now.
    pub(crate) fn refile_workspace(&mut self, cx: &mut Context<Self>) {
        self.files.tabs.clear();
        self.files.active = None;
        self.files.expanded.clear();
        self.files.children.clear();
        self.files.roots.clear();
        self.files.error = None;
        self.files.renaming = None;
        self.files.deleting = None;
        self.files.new_file_open = false;
        self.files.notes_open = false;
        self.files.focus_note = None;
        self.files.thread_quote = None;
        self.files.last_blank_press = None;
        if self.right.tab == Some(RightTab::Files) {
            // This also reloads the notes, which travel with the workspace.
            self.refresh_files(cx);
            return;
        }
        self.files.notes = crate::notes::Notes::default();
        cx.notify();
    }

    /// Re-list the workspace root and every directory the reader expanded.
    ///
    /// A tree with N branches costs N walks, but only on an explicit refresh or
    /// after a mutation, never on a keystroke - the listings between those
    /// points come from `children`.
    pub(crate) fn refresh_files(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.state.workspace_root() else {
            self.files.roots.clear();
            self.files.children.clear();
            self.files.expanded.clear();
            self.files.notes = crate::notes::Notes::default();
            self.files.error = None;
            cx.notify();
            return;
        };
        // Notes travel with the workspace, so a root change reloads them; the
        // file reads are a single small JSON, and this runs on an explicit
        // refresh or a mutation, never on a frame.
        self.files.notes = crate::notes::Notes::load(&root);
        match files::list(&root, self.files.hidden) {
            Ok(rows) => {
                self.files.roots = rows;
                self.files.error = None;
            }
            Err(error) => self.files.error = Some(io_msg(&error)),
        }
        // Re-walk exactly the branches that are open, leaving the rest alone:
        // a collapsed branch's listing is stale but invisible, and paying for
        // it would be work nobody can see.
        let expanded = self.files.expanded.clone();
        for dir in expanded {
            match files::list(&root.join(&dir), self.files.hidden) {
                Ok(rows) => {
                    self.files.children.insert(dir, rows);
                }
                Err(error) => self.files.error = Some(io_msg(&error)),
            }
        }
        cx.notify();
    }

    /// Re-list the directories a mutation touched, and nothing else.
    fn refresh_branch(&mut self, dir: &Path, cx: &mut Context<Self>) {
        let Some(root) = self.state.workspace_root() else {
            return;
        };
        match files::list(&root.join(dir), self.files.hidden) {
            Ok(rows) => {
                self.files.children.insert(dir.to_path_buf(), rows);
            }
            Err(error) => self.files.error = Some(io_msg(&error)),
        }
        // The root listing is what shows a file created at the top level, and
        // it is one small walk, so refresh it with the branch.
        match files::list(&root, self.files.hidden) {
            Ok(rows) => self.files.roots = rows,
            Err(error) => self.files.error = Some(io_msg(&error)),
        }
        cx.notify();
    }

    /// Expand a directory, listing it on first open, or collapse it.
    ///
    /// Collapsing keeps the cached listing: reopening a branch the reader just
    /// looked at should not cost a second walk.
    pub(crate) fn toggle_dir(&mut self, rel: &Path, cx: &mut Context<Self>) {
        if let Some(index) = self.files.expanded.iter().position(|p| p == rel) {
            self.files.expanded.remove(index);
            cx.notify();
            return;
        }
        let already_listed = self.files.children.contains_key(rel);
        if !already_listed {
            let Some(root) = self.state.workspace_root() else {
                return;
            };
            match files::list(&root.join(rel), self.files.hidden) {
                Ok(rows) => {
                    self.files.children.insert(rel.to_path_buf(), rows);
                }
                Err(error) => {
                    self.files.error = Some(io_msg(&error));
                    cx.notify();
                    return;
                }
            }
        }
        self.files.expanded.push(rel.to_path_buf());
        cx.notify();
    }

    /// Send the file's open comments to the model as one review.
    ///
    /// This is the deliberate batch - the drawer's button, and the card's own
    /// Comment action right after it pins the remark the reader just wrote.
    /// Notes that already went out are skipped, so a second comment's review
    /// asks about the new comment instead of repeating the whole file's
    /// history at the model.
    pub(crate) fn send_review_to_chat(
        &mut self,
        rel: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let indices: Vec<usize> = self
            .files
            .notes
            .on(rel)
            .iter()
            .enumerate()
            .filter(|(_, note)| !note.resolved)
            .map(|(index, _)| index)
            .collect();
        if indices.is_empty() {
            return;
        }
        self.send_notes(rel, &indices, window, cx);
        self.files.notes_open = false;
        self.files.comment_open = false;
    }

    /// Send only the remarks this file has not sent before.
    ///
    /// The card's own Comment action uses this: the reader just wrote ONE
    /// remark, and re-asking about every earlier one on the file would spend a
    /// turn repeating a conversation the model already had. The drawer's
    /// explicit Send is the other shape - it means the whole open review.
    pub(crate) fn send_unsent_notes(
        &mut self,
        rel: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let indices: Vec<usize> = self
            .files
            .notes
            .on(rel)
            .iter()
            .enumerate()
            .filter(|(_, note)| !note.resolved && !note.sent)
            .map(|(index, _)| index)
            .collect();
        if indices.is_empty() {
            return;
        }
        self.send_notes(rel, &indices, window, cx);
        self.files.notes_open = false;
        self.files.comment_open = false;
        cx.notify();
    }

    /// Ask the model about one comment: the thread panel's own action.
    pub(crate) fn ask_comment(
        &mut self,
        rel: &Path,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.send_notes(rel, &[index], window, cx);
        cx.notify();
    }

    /// Deliver the given notes to the model and record that they went out.
    ///
    /// The request is sent as a turn of its own, not parked in the composer:
    /// pressing Ask on a comment is a question about this file, and the
    /// reader's draft is left exactly where it was because it is not about this
    /// comment. A refusal (a stale model, a locked vault) is the one case that
    /// lands in the composer instead, so the words are not lost just because
    /// the turn could not open.
    fn send_notes(
        &mut self,
        rel: &Path,
        indices: &[usize],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self
            .files
            .active_tab()
            .filter(|tab| tab.rel == rel)
            .and_then(|tab| tab.editor.clone())
            .map(|editor| editor.read(cx).value().to_string())
            .unwrap_or_default();
        let notes = self.files.notes.on(rel);
        let selected: Vec<crate::notes::Note> = indices
            .iter()
            .filter_map(|index| notes.get(*index).cloned())
            .collect();
        let prompt = crate::notes::review_prompt(rel, &selected, &text);
        if prompt.trim().is_empty() {
            return;
        }
        if self.deliver_request(prompt, window, cx) {
            self.files.notes.mark_sent(rel, indices);
            self.save_notes();
            self.files.focus_note = indices.last().copied().or(self.files.focus_note);
        }
        cx.notify();
    }

    /// The scope a comment would take right now: the selected lines and the
    /// code they cover.
    ///
    /// One definition for the card's label, the note that gets pinned and the
    /// quote stored with it, so the three cannot disagree about which lines a
    /// remark is about or what it quoted.
    pub(crate) fn note_scope(&self, cx: &App) -> (Option<usize>, Option<usize>, String) {
        let Some(editor) = self.files.active_tab().and_then(|tab| tab.editor.clone()) else {
            return (None, None, String::new());
        };
        let state = editor.read(cx);
        let range = state.selected_range();
        let text = state.value();
        let from = crate::notes::line_at_offset(text.as_ref(), range.start);
        let end = crate::notes::line_at_offset(text.as_ref(), range.end);
        let to = (end > from).then_some(end);
        let quote = crate::notes::quote_of(text.as_ref(), from, end);
        (Some(from), to, quote)
    }

    /// Put a comment in focus and bring its line into view.
    pub(crate) fn focus_comment(&mut self, rel: &Path, index: usize, cx: &mut Context<Self>) {
        self.files.focus_note = Some(index);
        self.reveal_comment(rel, index, cx);
        // Read the code once, here, so every frame the panel draws its quote
        // without touching the buffer.
        let excerpt = self
            .files
            .active_tab()
            .filter(|tab| tab.rel == rel)
            .and_then(|tab| tab.editor.clone())
            .and_then(|editor| {
                self.files
                    .notes
                    .on(rel)
                    .get(index)
                    .map(|note| crate::notes::excerpt(note, editor.read(cx).value().as_ref()))
            })
            .unwrap_or_default();
        self.files.thread_quote = Some((rel.to_path_buf(), index, excerpt));
        cx.notify();
    }

    /// Scroll the document to a comment's anchor and mark its lines.
    ///
    /// Selecting the range is what makes the code the comment is about
    /// obvious after the reader jumped from the drawer; it also puts the
    /// keyboard where the reader was reading.
    pub(crate) fn reveal_comment(&mut self, rel: &Path, index: usize, cx: &mut Context<Self>) {
        let Some(note) = self.files.notes.on(rel).get(index).cloned() else {
            return;
        };
        // A jump selects source lines: the rendered view has none to select.
        self.set_preview(rel, false, cx);
        let Some(line) = note.line else {
            return;
        };
        let end = note.end().unwrap_or(line);
        let Some(editor) = self
            .files
            .active_tab()
            .filter(|tab| tab.rel == rel)
            .and_then(|tab| tab.editor.clone())
        else {
            return;
        };
        editor.update(cx, |state, cx| {
            let text = state.value();
            let start = crate::notes::line_start_offset(text.as_ref(), line);
            let stop = crate::notes::line_start_offset(text.as_ref(), end.saturating_add(1));
            let stop = stop.min(text.len());
            if stop > start {
                state.set_selected_range(start..stop, cx);
            }
            let line_height = f32::from(state.line_height().unwrap_or(px(20.0)));
            // Three lines of context above the anchor, so the mark the reader
            // asked for is not glued to the viewport's top edge.
            let top = (line.saturating_sub(1)) as f32 * line_height - line_height * 3.0;
            state.set_scroll_offset(gpui_kit::point(px(0.0), px(-top.max(0.0))), cx);
        });
    }

    /// Flip one comment between open and resolved.
    pub(crate) fn resolve_comment(&mut self, rel: &Path, index: usize, cx: &mut Context<Self>) {
        let Some(resolved) = self
            .files
            .notes
            .on(rel)
            .get(index)
            .map(|note| note.resolved)
        else {
            return;
        };
        self.files.notes.set_resolved(rel, index, !resolved);
        self.save_notes();
        self.sync_note_marks(rel, cx);
        cx.notify();
    }

    /// Re-point the editor's comment marks at a file's notes.
    ///
    /// The marks are a range-decoration collection, which tracks the buffer's
    /// own edits: typing above a comment moves its tint with the code without
    /// this being called. It is called when the NOTES change - a pin, a delete,
    /// a resolve, a re-anchor after a save - because that is when the ranges
    /// themselves are redefined.
    fn sync_note_marks(&mut self, rel: &Path, cx: &mut Context<Self>) {
        let Some(index) = self.files.tab_index(rel) else {
            return;
        };
        let Some(editor) = self.files.tabs[index].editor.clone() else {
            return;
        };
        if self.files.notes.on(rel).is_empty() && self.files.tabs[index].decorations.is_none() {
            return;
        }
        let mut marks = Vec::new();
        let mut owners = Vec::new();
        {
            let text = editor.read(cx).value();
            for (note_index, note) in self.files.notes.on(rel).iter().enumerate() {
                let Some(from) = note.line else {
                    continue;
                };
                let to = note.end().unwrap_or(from);
                let start = crate::notes::line_start_offset(text.as_ref(), from);
                let end = crate::notes::line_start_offset(text.as_ref(), to.saturating_add(1));
                let end = end.min(text.len());
                if end <= start {
                    continue;
                }
                let color = if note.resolved {
                    Hsla::from(rgb(theme::muted())).alpha(0.06)
                } else {
                    Hsla::from(rgb(theme::accent())).alpha(0.10)
                };
                marks.push(
                    RangeDecoration::new(start..end)
                        .with_style(RangeDecorationStyle::Fill)
                        .with_color(color),
                );
                owners.push(note_index);
            }
        }
        let collection = match self.files.tabs[index].decorations.clone() {
            Some(collection) => collection,
            None => {
                let created = editor.update(cx, |state, cx| {
                    state.create_range_decorations_collection(Vec::new(), cx)
                });
                self.files.tabs[index].decorations = Some(created.clone());
                created
            }
        };
        self.files.tabs[index].note_marks = owners;
        collection.set(marks, cx);
    }

    /// Persist the notes, reporting a write that failed where the pane does.
    fn save_notes(&mut self) {
        let Some(root) = self.state.workspace_root() else {
            return;
        };
        if let Err(error) = self.files.notes.save(&root) {
            self.files.error = Some(io_msg(&error));
        }
    }

    /// Record one workspace activation in the launcher's automatic recent list.
    pub(crate) fn record_workspace_recent(&mut self, id: &str) {
        let (label, root) = self
            .state
            .settings
            .global
            .workspaces
            .iter()
            .find(|workspace| workspace.id == id)
            .map(|workspace| (workspace.name.clone(), workspace.root.clone()))
            .unwrap_or_else(|| (id.to_string(), String::new()));
        self.state.record_launcher_recent(
            crate::recent::RecentKind::Workspace,
            id,
            label,
            crate::recent::display_path(&root),
            Some(id.to_string()),
        );
    }

    /// Record one document opening in the launcher's automatic recent list.
    fn record_document_recent(&mut self, rel: &Path) {
        let id = rel.display().to_string();
        let detail = self
            .state
            .workspace_root()
            .map(|root| root.join(rel))
            .map(|path| crate::recent::display_path(&path.to_string_lossy()))
            .unwrap_or_else(|| crate::recent::display_path(&id));
        let label = rel
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| detail.clone());
        let workspace = self.state.settings.global.general.active_workspace.clone();
        self.state.record_launcher_recent(
            crate::recent::RecentKind::Document,
            id,
            label,
            detail,
            (!workspace.is_empty()).then_some(workspace),
        );
    }

    /// Open a file in the editor area as a tab.
    ///
    /// A file already open is focused rather than reopened, so the strip never
    /// carries the same file twice and a buffer with unsaved changes is never
    /// thrown away by clicking its row again. The read is bounded and the
    /// format decides the body: a text file lands in a highlighted editor, an
    /// image lands in an image pane, and anything else says it cannot be shown
    /// rather than dropping bytes into a buffer.
    pub(crate) fn open_file(&mut self, rel: &Path, window: &mut Window, cx: &mut Context<Self>) {
        // A document's body only exists on the Files tab, so opening one from
        // anywhere else (a chat reference, a search result, the Start grid) has
        // to bring that tab up with it. This is the invariant the strip's file
        // chips rely on: every open file is a tab you can see and return to.
        self.show_right_tab(RightTab::Files, cx);
        if let Some(index) = self.files.tab_index(rel) {
            if self.files.active != Some(index) {
                // The focused thread belongs to the document that was on
                // screen, and this is a different one.
                self.files.focus_note = None;
                self.files.thread_quote = None;
            }
            self.files.active = Some(index);
            // Reopening a tab shows its remarks if it has any; a file with
            // none keeps the surface clean until the reader opens the drawer.
            self.files.notes_open = self.files.notes.has_notes(rel);
            self.record_document_recent(rel);
            cx.notify();
            return;
        }
        let Some(root) = self.state.workspace_root() else {
            return;
        };
        let Some(path) = files::resolve(&root, rel) else {
            self.files.error = Some(self.state.lang.tr("right.files.outside"));
            cx.notify();
            return;
        };
        self.record_document_recent(rel);
        // The name decides the language and the size decides the bound, so both
        // come from the resolved path rather than from the label on the row.
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let size = path.metadata().map(|meta| meta.len()).unwrap_or(0);
        let kind = files::kind_for(&name, size);
        let mut tab = FileTab {
            rel: rel.to_path_buf(),
            editor: None,
            kind,
            dirty: false,
            decorations: None,
            note_marks: Vec::new(),
            preview: files::is_markdown(rel) || files::is_svg(rel),
            document: None,
            svg: None,
        };
        if !matches!(kind, Kind::Text(_)) {
            self.files.tabs.push(tab);
            self.files.active = Some(self.files.tabs.len() - 1);
            self.files.notes_open = self.files.notes.has_notes(rel);
            self.files.error = None;
            cx.notify();
            return;
        }
        match files::read(&root, rel) {
            Ok(bytes) => {
                // A file with a text extension that is not really text falls
                // back to the opaque body, so the buffer never holds bytes the
                // editor would render as control characters.
                if !files::is_text(&bytes) {
                    tab.kind = Kind::Opaque;
                    self.files.tabs.push(tab);
                    self.files.active = Some(self.files.tabs.len() - 1);
                    self.files.notes_open = self.files.notes.has_notes(rel);
                    cx.notify();
                    return;
                }
                let Ok(text) = String::from_utf8(bytes) else {
                    tab.kind = Kind::Opaque;
                    self.files.tabs.push(tab);
                    self.files.active = Some(self.files.tabs.len() - 1);
                    self.files.notes_open = self.files.notes.has_notes(rel);
                    cx.notify();
                    return;
                };
                let language = files::language_for(&name);
                let editor = cx.new(|cx| {
                    let mut state = EditorState::new(window, cx).searchable(true);
                    if let Some(language) = language {
                        state = state.language(language);
                    }
                    state.set_value(text, window, cx);
                    state
                });
                let editor_id = editor.clone();
                let open_rel = rel.to_path_buf();
                cx.subscribe_in(
                    &editor,
                    window,
                    move |this, editor_source, event, _window, cx| {
                        if matches!(event, InputEvent::Change) {
                            // The dirty mark belongs to the tab holding this exact
                            // editor, so a second tab over another file is never
                            // marked by the first one's keystrokes.
                            if let Some(tab) = this.files.tabs.iter_mut().find(|tab| {
                                tab.rel == open_rel && tab.editor.as_ref() == Some(&editor_id)
                            }) {
                                if !tab.dirty {
                                    tab.dirty = true;
                                    cx.notify();
                                }
                            }
                            // A keystroke can move the focused comment's code, so
                            // the cached excerpt is refreshed here, once per edit,
                            // instead of by a frame reading the file.
                            if let Some(index) = this.files.focus_note {
                                let note = this.files.notes.on(&open_rel).get(index).cloned();
                                if let Some(note) = note {
                                    let text = editor_source.read(cx).value();
                                    this.files.thread_quote = Some((
                                        open_rel.clone(),
                                        index,
                                        crate::notes::excerpt(&note, text.as_ref()),
                                    ));
                                }
                            }
                        }
                    },
                )
                .detach();
                // The buffer is read once here: it seeds the Markdown preview
                // and settles the comment anchors against the text the reader
                // is about to see.
                let buffer = editor.read(cx).value().to_string();
                if files::is_svg(rel) {
                    // Rasterized at open, from the buffer the editor holds: the
                    // preview shows the document that was read, and a rebuild on
                    // the next preview shows an edit.
                    match svg_preview(buffer.as_bytes(), size) {
                        Some(svg) => tab.svg = Some(svg),
                        None => {
                            tab.preview = false;
                            self.files.error = Some(self.state.lang.tr("right.files.svgFailed"));
                        }
                    }
                } else if tab.preview {
                    // Parsed at open: a reader who opens a .md file asked for
                    // the rendered document, so the first frame has it.
                    tab.document = Some(cx.new(|cx| TextViewState::markdown(buffer.as_str(), cx)));
                }
                tab.editor = Some(editor);
                self.files.tabs.push(tab);
                self.files.active = Some(self.files.tabs.len() - 1);
                self.files.focus_note = None;
                self.files.thread_quote = None;
                self.files.notes_open = self.files.notes.has_notes(rel);
                self.files.error = None;
                if self.files.notes.reanchor(rel, &buffer) {
                    self.save_notes();
                }
                self.sync_note_marks(rel, cx);
                cx.notify();
            }
            Err(error) => {
                self.files.error = Some(io_msg(&error));
                cx.notify();
            }
        }
    }

    /// Focus a tab by index, which is what pressing its chip does.
    ///
    /// A document only has a body while Files is the pane, so a chip pressed
    /// from another pane brings Files back with it. The reference's dock does
    /// the same: selecting a document tab is selecting the surface that draws
    /// it, not merely recolouring the chip.
    pub(crate) fn focus_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.files.tabs.len() {
            if self.files.active != Some(index) {
                self.files.focus_note = None;
                self.files.thread_quote = None;
            }
            self.show_right_tab(RightTab::Files, cx);
            self.files.active = Some(index);
            cx.notify();
        }
    }

    /// Show the tree again, which is what pressing the Files chip does.
    pub(crate) fn focus_tree(&mut self, cx: &mut Context<Self>) {
        self.files.active = None;
        self.files.focus_note = None;
        self.files.thread_quote = None;
        self.right.catalog_open = false;
        cx.notify();
    }

    /// Close a tab and leave the neighbour that was beside it active.
    ///
    /// Finder hands the body to the tab that takes the closed one's place -
    /// its right neighbour, or its left when it was the last. bsh used to drop
    /// the reader back to the tree instead, which made closing a tab feel like
    /// closing the pane; the tree is the destination only when no document is
    /// left.
    pub(crate) fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.files.tabs.len() {
            return;
        }
        let was_active = self.files.active == Some(index);
        self.files.tabs.remove(index);
        self.files.focus_note = None;
        self.files.thread_quote = None;
        self.files.active = if was_active {
            if self.files.tabs.is_empty() {
                None
            } else {
                Some(index.min(self.files.tabs.len() - 1))
            }
        } else {
            match self.files.active {
                Some(open) if open > index => Some(open - 1),
                other => other,
            }
        };
        cx.notify();
    }

    /// Close every document tab except the one at `keep`.
    ///
    /// The kept tab becomes the only tab, so it takes index 0 and the body.
    /// Finder's "Close Other Tabs" is this action; the pane stays where it is.
    pub(crate) fn close_other_tabs(&mut self, keep: usize, cx: &mut Context<Self>) {
        if keep >= self.files.tabs.len() {
            return;
        }
        let kept = self.files.tabs.remove(keep);
        self.files.tabs.clear();
        self.files.tabs.push(kept);
        self.files.active = Some(0);
        // "Other tabs" includes the pane tabs, not only the documents: the
        // kept file keeps the Files pane, every other pane closes.
        let had_browser = self.right.open_panes.contains(&RightTab::Browser);
        let had_terminal = self.right.open_panes.contains(&RightTab::Terminal);
        self.right
            .open_panes
            .retain(|pane| *pane == RightTab::Files);
        if !self.right.open_panes.contains(&RightTab::Files) {
            self.right.open_panes.push(RightTab::Files);
        }
        if had_browser {
            self.browser_pane_closed();
        }
        if had_terminal {
            self.hide_terminal_view(cx);
        }
        self.activate_pane(RightTab::Files, cx);
    }

    /// Close every tab, documents and panes alike, and return to Start.
    pub(crate) fn close_all_tabs(&mut self, cx: &mut Context<Self>) {
        self.files.tabs.clear();
        self.files.active = None;
        self.right.open_panes.clear();
        self.right.tab = None;
        self.right.catalog_open = false;
        self.browser_pane_closed();
        self.hide_terminal_view(cx);
        cx.notify();
    }

    /// Show the Start screen without closing the tabs that are open.
    ///
    /// The plus is a new-tab page, not "close the pane": every pane and document
    /// behind it stays a tab, and choosing one brings it back.
    pub(crate) fn show_start(&mut self, cx: &mut Context<Self>) {
        self.right.tab = None;
        self.right.catalog_open = false;
        self.hide_terminal_view(cx);
        cx.notify();
    }

    /// Close one pane tab and activate its neighbour, or Start when none is
    /// left. Documents stay as tabs.
    pub(crate) fn close_right_pane_tab(&mut self, tab: RightTab, cx: &mut Context<Self>) {
        let was_active = self.right.tab == Some(tab);
        let active_before = self.right.tab;
        let index = self.right.open_panes.iter().position(|pane| *pane == tab);
        self.right.open_panes.retain(|pane| *pane != tab);
        if tab == RightTab::Browser {
            // This also clears the frame and the engine; it sets tab to None,
            // so an inactive close has to put the pane that is still up back.
            self.browser_pane_closed();
        }
        if !was_active {
            self.right.tab = active_before;
            cx.notify();
            return;
        }
        let next = index.and_then(|index| {
            self.right
                .open_panes
                .get(index)
                .or_else(|| self.right.open_panes.get(index.saturating_sub(1)))
                .copied()
        });
        self.right.catalog_open = false;
        match next {
            Some(next) => self.activate_pane(next, cx),
            None => {
                self.right.tab = None;
                self.hide_terminal_view(cx);
                cx.notify();
            }
        }
    }

    /// Close every tab except one pane, which is the pane tab's "Close Other
    /// Tabs": other panes and every document go, the chosen pane stays.
    pub(crate) fn close_other_pane_tabs(&mut self, keep: RightTab, cx: &mut Context<Self>) {
        self.right.open_panes.retain(|pane| *pane == keep);
        self.files.tabs.clear();
        self.files.active = None;
        if keep != RightTab::Browser {
            self.browser_pane_closed();
        }
        if keep != RightTab::Terminal {
            self.hide_terminal_view(cx);
        }
        self.activate_pane(keep, cx);
    }

    /// Close the tab on screen: the active document, or the active pane.
    ///
    /// Start is the new-tab page and has no close of its own, so pressing Close
    /// there is a no-op (the menu disables it).
    pub(crate) fn close_active_tab(&mut self, cx: &mut Context<Self>) {
        match self.right.tab {
            Some(RightTab::Files) if self.files.active.is_some() => {
                if let Some(index) = self.files.active {
                    self.close_tab(index, cx);
                }
            }
            Some(tab) => self.close_right_pane_tab(tab, cx),
            None => {}
        }
    }

    /// Keep the tab on screen and close every other pane and document tab.
    pub(crate) fn close_other_active_tabs(&mut self, cx: &mut Context<Self>) {
        match self.right.tab {
            Some(RightTab::Files) if self.files.active.is_some() => {
                if let Some(keep) = self.files.active {
                    self.close_other_tabs(keep, cx);
                }
            }
            Some(tab) => self.close_other_pane_tabs(tab, cx),
            None => {}
        }
    }

    /// Show Finder's Show All Tabs catalogue over the panel body.
    ///
    /// The terminal and the browser paint on native layers above this tree, so
    /// the terminal surface is hidden for the duration; the browser is hidden
    /// by its own visibility rule while the catalogue is up. Both are restored
    /// by close_tab_catalog or by the tab a card chooses.
    pub(crate) fn open_tab_catalog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.right.tab.is_none()
            && self.right.open_panes.is_empty()
            && self.files.tabs.is_empty()
        {
            return;
        }
        self.right.catalog_open = true;
        self.hide_terminal_view(cx);
        // A fresh overview is the whole catalogue: an old query and an old
        // category would hide the very tab the reader just asked to find.
        self.catalog_filter = CatalogFilter::All;
        self.catalog_query_text.clear();
        self.catalog_query.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus_handle(cx).focus(window, cx);
        });
        cx.notify();
    }

    /// Put the body back after the catalogue closes.
    pub(crate) fn close_tab_catalog(&mut self, cx: &mut Context<Self>) {
        if !self.right.catalog_open {
            return;
        }
        self.right.catalog_open = false;
        // The terminal surface is a window layer, not part of this tree; hiding
        // it for the overview means it has to be brought back explicitly when
        // the pane it belongs to is still the one on screen.
        if self.right.tab == Some(RightTab::Terminal) {
            if let Some(view) = self.right.terminal_view.clone() {
                view.update(cx, |view, _| view.set_visible(true));
            }
        }
        cx.notify();
    }

    /// The right-pane items the top search box matches.
    ///
    /// The search covers the same things the pane offers: its seven surfaces
    /// (Files, Browser, Extensions and the rest) and every document opened out
    /// of Files. A document matches on its name or its workspace-relative path,
    /// so a search for a directory still finds the file inside it.
    pub(crate) fn right_search_matches(&self) -> Vec<(RightSearchEntry, String)> {
        let query = self.right_search_text.trim().to_lowercase();
        let mut matches = Vec::new();
        if self.right_search_scope != RightSearchScope::Documents {
            for tab in RightTab::ALL {
                let label = self.state.lang.tr(tab.key());
                if query.is_empty() || label.to_lowercase().contains(&query) {
                    matches.push((RightSearchEntry::Pane(tab), label));
                }
            }
        }
        if self.right_search_scope != RightSearchScope::Panes {
            for (index, tab) in self.files.tabs.iter().enumerate() {
                let label = tab.label();
                let rel = tab.rel.display().to_string();
                if query.is_empty()
                    || label.to_lowercase().contains(&query)
                    || rel.to_lowercase().contains(&query)
                {
                    matches.push((RightSearchEntry::File(index), label));
                }
            }
        }
        matches
    }

    /// Clear the top search field and close its results.
    pub(crate) fn clear_right_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.right_search_text.clear();
        self.right_search
            .update(cx, |state, cx| state.set_value("", window, cx));
        cx.notify();
    }

    /// Open one search result: a pane, or the document a file row names.
    pub(crate) fn open_right_search_result(
        &mut self,
        entry: RightSearchEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clear_right_search(window, cx);
        match entry {
            RightSearchEntry::Pane(tab) => self.open_right_tab(tab, window, cx),
            RightSearchEntry::File(index) => self.focus_tab(index, cx),
        }
    }

    /// Enter in the search field opens the first match, the way Finder's own
    /// search opens the selection when Return is pressed.
    /// Bring the Workspace Launcher forward and put the caret in its search.
    pub(crate) fn focus_launcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.right.open = true;
        self.right.tab = None;
        self.right.catalog_open = false;
        self.right_search
            .update(cx, |state, cx| state.focus_handle(cx).focus(window, cx));
        cx.notify();
    }

    /// Open the first launcher result, including when the search is empty.
    ///
    /// Enter in the search field requires a query (an empty Enter should do
    /// nothing), but the footer's Open is the launcher's primary action: with
    /// no query it opens the first pane card, which is Files.
    pub(crate) fn open_first_launcher_item(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.right.tab.is_some() {
            return;
        }
        if let Some((entry, _)) = self.right_search_matches().into_iter().next() {
            self.clear_right_search(window, cx);
            self.open_right_search_result(entry, window, cx);
            return;
        }
        if let Some(entry) = self.launcher_recent_rows().into_iter().next() {
            self.open_launcher_recent(entry, window, cx);
        }
    }

    pub(crate) fn open_first_right_search_result(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The field lives on the main right pane only; with a pane open there is
        // no visible search to press Enter in.
        if self.right.tab.is_some() || self.right_search_text.trim().is_empty() {
            return;
        }
        let Some((entry, _)) = self.right_search_matches().into_iter().next() else {
            return;
        };
        self.open_right_search_result(entry, window, cx);
    }

    /// Write the active tab's buffer back to the file it came from.
    pub(crate) fn save_file(&mut self, rel: &Path, cx: &mut Context<Self>) {
        let Some(root) = self.state.workspace_root() else {
            return;
        };
        let Some(index) = self.files.tab_index(rel) else {
            return;
        };
        let Some(editor) = self.files.tabs[index].editor.clone() else {
            return;
        };
        let text = editor.read(cx).value().to_string();
        match files::write(&root, rel, &text) {
            Ok(()) => {
                // Only the saved tab clears, so a second tab over another file
                // keeps its own unsaved marker.
                self.files.tabs[index].dirty = false;
                self.files.error = None;
                // The buffer is now the file, so the notes can follow an edit
                // that moved their lines: the saved text is the authority the
                // re-anchor reads.
                if self.files.notes.reanchor(rel, &text) {
                    self.save_notes();
                }
                self.sync_note_marks(rel, cx);
            }
            Err(error) => self.files.error = Some(io_msg(&error)),
        }
        cx.notify();
    }

    /// Pin a note to the document the panel is showing.
    ///
    /// The note anchors to the editor's own cursor line, so a reader can place
    /// a remark exactly where they are looking; a tab with no editor (an
    /// image, or a file that will not open) takes a whole-file note instead.
    pub(crate) fn pin_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self
            .files
            .note_text
            .read(cx)
            .value()
            .to_string()
            .trim()
            .to_string();
        if text.is_empty() {
            return;
        }
        let Some(rel) = self.files.active_tab().map(|tab| tab.rel.clone()) else {
            return;
        };
        // The SELECTION is the anchor: a note about three lines says three lines,
        // the way the reference's card reports them.
        // The scope carries the code as well as the lines: the quote is what
        // lets the note follow its code after an edit, and what the panel shows
        // the reader is commenting on.
        let (line, to, quote) = self.note_scope(cx);
        let shown = quote.clone();
        self.files
            .notes
            .add(&rel, crate::notes::Note::new(line, to, text, quote));
        self.save_notes();
        self.sync_note_marks(&rel, cx);
        self.files.note_text.update(cx, |state, cx| {
            state.set_value(String::new(), window, cx);
        });
        // The remark the reader just wrote is the one they want to read back:
        // focus it in the thread panel instead of covering the code with a
        // drawer.
        let index = self.files.notes.on(&rel).len().saturating_sub(1);
        self.files.focus_note = Some(index);
        self.files.thread_quote = Some((rel.clone(), index, shown));
        cx.notify();
    }

    /// Remove one note from the document, by its index in the drawer.
    pub(crate) fn delete_note(&mut self, rel: &Path, index: usize, cx: &mut Context<Self>) {
        self.files.notes.remove(rel, index);
        self.save_notes();
        // The panel must not keep pointing at a comment that is gone, and a
        // later note's index has shifted if it sat after the removed one.
        self.files.focus_note = match self.files.focus_note {
            Some(focus) if focus > index => Some(focus - 1),
            Some(focus) if focus == index => None,
            other => other,
        };
        self.files.thread_quote = match self.files.thread_quote.take() {
            Some((cached_rel, cached_index, text)) if cached_rel == rel => {
                if cached_index > index {
                    Some((cached_rel, cached_index - 1, text))
                } else if cached_index == index {
                    None
                } else {
                    Some((cached_rel, cached_index, text))
                }
            }
            other => other,
        };
        self.sync_note_marks(rel, cx);
        cx.notify();
    }

    /// Create the file named in the New file field.
    ///
    /// A blank name is a no-op rather than an error, because the field is
    /// always on screen and a reader tabbing past it should not be told off.
    pub(crate) fn create_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.state.workspace_root() else {
            return;
        };
        let name = self
            .files
            .new_name
            .read(cx)
            .value()
            .to_string()
            .trim()
            .to_string();
        if name.is_empty() {
            return;
        }
        match files::create(&root, Path::new(&name)) {
            Ok(_path) => {
                self.files.new_name.update(cx, |state, cx| {
                    state.set_value(String::new(), window, cx);
                });
                // The file exists, so the field that asked for it retires.
                self.files.new_file_open = false;
                self.refresh_branch(Path::new(""), cx);
            }
            Err(error) => {
                self.files.error = Some(io_msg(&error));
                cx.notify();
            }
        }
    }

    /// Open the new-file field after a double-click on the tree's blank space.
    ///
    /// There is no permanent create row in the reference, and a modal dialog
    /// would block a reader who only meant to clear the selection. So the
    /// gesture simply reveals the inline field and hands it the keyboard: the
    /// reader types a name and presses the button (or Enter) to make the file,
    /// and a single click elsewhere dismisses it.
    pub(crate) fn blank_double_clicked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.files.renaming = None;
        self.files.new_file_open = true;
        let focus = self.files.new_name.clone();
        focus.update(cx, |state, cx| {
            state.focus_handle(cx).focus(window, cx);
        });
        cx.notify();
    }

    /// Arm a row's delete: the first press marks it, the second deletes.
    pub(crate) fn press_delete(&mut self, rel: &Path, cx: &mut Context<Self>) {
        if self.files.deleting.as_deref() == Some(rel) {
            self.delete_file(rel, cx);
            return;
        }
        self.files.deleting = Some(rel.to_path_buf());
        cx.notify();
    }

    /// Delete the row, closing any tab showing it so no buffer points at a
    /// file that is gone.
    pub(crate) fn delete_file(&mut self, rel: &Path, cx: &mut Context<Self>) {
        let Some(root) = self.state.workspace_root() else {
            return;
        };
        match files::delete(&root, rel) {
            Ok(()) => {
                self.files.deleting = None;
                // The remarks described a file that is gone, so they go with it
                // rather than resurfacing if a second file takes the name.
                self.files.notes.forget(rel);
                if let Err(error) = self.files.notes.save(&root) {
                    self.files.error = Some(io_msg(&error));
                }
                // Walk backwards so an index the removal shifts never skips a
                // tab over the same file.
                let mut index = self.files.tabs.len();
                while index > 0 {
                    index -= 1;
                    if self.files.tabs[index].rel == rel {
                        self.close_tab(index, cx);
                    }
                }
                let parent = rel.parent().unwrap_or(Path::new("")).to_path_buf();
                self.refresh_branch(&parent, cx);
            }
            Err(error) => {
                self.files.error = Some(io_msg(&error));
                cx.notify();
            }
        }
    }

    /// Put a row into the rename field, prefilled with the name it has.
    pub(crate) fn begin_rename(&mut self, rel: &Path, window: &mut Window, cx: &mut Context<Self>) {
        self.files.renaming = Some(rel.to_path_buf());
        let name = rel
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        self.files.rename_name.update(cx, |state, cx| {
            state.set_value(name, window, cx);
            // The reader pressed the pencil, so they mean to type: focus the
            // field and select nothing, the way the create field opens.
            state.focus_handle(cx).focus(window, cx);
        });
        self.files.deleting = None;
        cx.notify();
    }

    /// Apply the rename, reporting a refusal rather than a silent no-op.
    pub(crate) fn rename_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.state.workspace_root() else {
            return;
        };
        let Some(from_rel) = self.files.renaming.clone() else {
            return;
        };
        let to = self
            .files
            .rename_name
            .read(cx)
            .value()
            .to_string()
            .trim()
            .to_string();
        if to.is_empty() || Some(to.as_str()) == from_rel.file_name().and_then(|n| n.to_str()) {
            self.files.renaming = None;
            self.files.rename_name.update(cx, |state, cx| {
                state.set_value(String::new(), window, cx);
            });
            cx.notify();
            return;
        }
        let to_rel = from_rel.with_file_name(&to);
        match files::rename(&root, &from_rel, &to_rel) {
            Ok(_path) => {
                self.files.renaming = None;
                self.files.rename_name.update(cx, |state, cx| {
                    state.set_value(String::new(), window, cx);
                });
                // An open tab over the old name follows it, so the reader does
                // not lose their buffer to a rename they made in the tree.
                if let Some(tab) = self.files.tabs.iter_mut().find(|tab| tab.rel == from_rel) {
                    tab.rel = to_rel.clone();
                }
                // Notes are keyed by the path, so they move with the file.
                self.files.notes.rename(&from_rel, &to_rel);
                if let Err(error) = self.files.notes.save(&root) {
                    self.files.error = Some(io_msg(&error));
                }
                self.refresh_branch(from_rel.parent().unwrap_or(Path::new("")), cx);
                if to_rel.parent() != from_rel.parent() {
                    self.refresh_branch(to_rel.parent().unwrap_or(Path::new("")), cx);
                }
            }
            Err(error) => {
                self.files.error = Some(io_msg(&error));
                cx.notify();
            }
        }
    }
}

/// The glyph a file is drawn with from its name alone, which is what a tab
/// chip needs: a tab carries a label, not a listing row.
/// The two-letter badge a tab shows for a file type, and its colour.
///
/// The reference draws a coloured file-type square on each document tab (MD,
/// RS, TS, ...). Naming the type in the tab is what makes a strip of similar
/// names scannable; a file the languages do not name keeps the generic glyph.
fn file_type_badge(name: &str) -> Option<(&'static str, u32)> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "md" | "markdown" | "mdx" => ("MD", 0x4A90E2),
        "rs" => ("RS", 0xC2410C),
        "ts" | "tsx" => ("TS", 0x2563EB),
        "js" | "jsx" | "mjs" | "cjs" => ("JS", 0xB7791F),
        "json" | "jsonc" => ("{}", 0x15803D),
        "toml" | "yaml" | "yml" | "ini" => ("CFG", 0x6B7280),
        "html" | "htm" => ("<>", 0xEA580C),
        "css" | "scss" | "less" => ("CSS", 0x0284C7),
        "sh" | "bash" | "zsh" => ("SH", 0x4D7C0F),
        "py" => ("PY", 0x2563EB),
        "go" => ("GO", 0x0891B2),
        "c" | "h" => ("C", 0x475569),
        "cpp" | "cc" | "hpp" => ("C+", 0x475569),
        "svg" | "png" | "jpg" | "jpeg" | "webp" | "gif" => ("IMG", 0x15803D),
        _ => return None,
    })
}

fn file_glyph_for_name(name: &str) -> IconName {
    if files::image_extension(name) {
        return IconName::Image;
    }
    if name.ends_with(".md") || name.ends_with(".markdown") {
        return IconName::BookOpen;
    }
    if name.ends_with(".lock") {
        return IconName::Lock;
    }
    match files::language_for(name) {
        Some(_) => IconName::FileCode,
        None => IconName::FileText,
    }
}

/// The glyph a row is drawn with, in the reference's own file-type language.
fn file_glyph(row: &files::Row) -> IconName {
    if row.dir {
        return IconName::Folder;
    }
    if files::image_extension(&row.name) {
        return IconName::Image;
    }
    let name = row.name.as_str();
    if name.ends_with(".md") || name.ends_with(".markdown") {
        return IconName::BookOpen;
    }
    if name.ends_with(".lock") {
        return IconName::Lock;
    }
    match files::language_for(name) {
        Some(_) => IconName::FileCode,
        None => IconName::FileText,
    }
}

/// Whether a tab has a rendered form the strip can switch to.
fn renders_preview(rel: &Path) -> bool {
    files::is_markdown(rel) || files::is_svg(rel)
}

/// The largest raster an SVG preview pays for, on its long side.
///
/// A retina window needs at most ~2x the pane; 1600 covers a wide preview
/// without paying for a poster-sized document the reader will never see at 1:1.
const SVG_RASTER_MAX: f32 = 1600.0;

/// Rasterize an SVG into the colour picture the pane paints.
///
/// GPUI's own `svg()` element renders an ALPHA MASK tinted with the text
/// colour, which is right for icons and wrong for a logo; resvg (the same
/// 0.46 crate the kit rasterizes with) gives the document's own colours. The
/// bytes come from the editor's buffer so an edited drawing previews its edit.
pub(crate) fn svg_preview(data: &[u8], bytes: u64) -> Option<SvgPreview> {
    let tree = resvg::usvg::Tree::from_data(data, &resvg::usvg::Options::default()).ok()?;
    let size = tree.size();
    let (width, height) = (size.width(), size.height());
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let scale = (SVG_RASTER_MAX / width.max(height)).clamp(0.01, 2.0);
    let raster_w = ((width * scale).ceil() as u32).max(1);
    let raster_h = ((height * scale).ceil() as u32).max(1);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(raster_w, raster_h)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let mut buffer = image::RgbaImage::new(raster_w, raster_h);
    for (source, target) in pixmap.pixels().iter().zip(buffer.pixels_mut()) {
        // tiny-skia hands back PREMULTIPLIED bytes while the image element
        // wants straight RGBA: skipping the division darkens every edge.
        let color = source.demultiply();
        *target = image::Rgba([color.red(), color.green(), color.blue(), color.alpha()]);
    }
    Some(SvgPreview {
        width: width.round().max(1.0) as u32,
        height: height.round().max(1.0) as u32,
        bytes,
        frame: std::sync::Arc::new(gpui_kit::RenderImage::new([image::Frame::new(buffer)])),
    })
}

/// What an image pane says under its picture: the decoded dimensions and the
/// file's size, read from the header rather than by decoding the whole image.
pub(crate) fn image_caption(path: &Path) -> Option<String> {
    let (width, height) = image::image_dimensions(path).ok()?;
    let size = path.metadata().ok()?.len();
    Some(format!("{width} × {height} · {}", size_label(size, false)))
}

/// The size a row shows: a directory has none, and a file's bytes are grouped
/// the way a file manager groups them.
fn size_label(size: u64, dir: bool) -> String {
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

/// The comparison surface's own chrome and controls, kept beside the pane they
/// belong to rather than in the panel's general body.
impl Shell {
    /// Open a workspace file as a comparison.
    ///
    /// The pane is reached by OPENING A FILE, not by a separate "pick a file"
    /// flow: a `.diff`/`.patch` opens as the patch it is, a conflicted file
    /// opens as the merge it is, and any other file opens against its committed
    /// text so the pane never shows a blank surface for a file the reader asked
    /// about.
    ///
    /// The READ happens on the runner's runtime, not here: a large file plus the
    /// two git subprocesses behind a real comparison are exactly the work that
    /// would stall the window on a click. The pane is focused first so the press
    /// has an immediate answer, then the result lands.
    pub(crate) fn open_diff(&mut self, rel: &Path, cx: &mut Context<Self>) {
        let Some(root) = self.state.workspace_root() else {
            self.diff_refused("right.files.noWorkspace", cx);
            return;
        };
        if files::resolve(&root, rel).is_none() {
            self.diff_refused("right.files.outside", cx);
            return;
        }
        // Answer the click now: the pane is where the result is going.
        self.show_right_tab(RightTab::Diff, cx);
        self.right.diff.error = None;
        cx.notify();
        let rel = rel.to_path_buf();
        // The GPUI background executor, not the runner's tokio runtime: this is
        // work the window started, and this executor is the one the test
        // scheduler can drive deterministically. A tokio runtime runs on threads
        // the harness refuses to see activity from.
        let loading = cx.background_executor().spawn({
            let root = root.clone();
            let rel = rel.clone();
            async move { diff_pane::read_for(&root, &rel) }
        });
        let label = rel.to_string_lossy().to_string();
        cx.spawn(async move |this, cx| {
            let loaded = loading.await;
            let _ = this.update(cx, |this, cx| match loaded {
                Ok(loaded) => match diff_pane::classify(
                    &label,
                    &loaded.bytes,
                    loaded.git.as_deref(),
                    loaded.untracked,
                ) {
                    Ok((parsed, source)) => {
                        this.present_comparison(&rel, parsed, source, cx);
                    }
                    Err(key) => this.diff_refused(key, cx),
                },
                Err(diff_pane::LoadFailure::Key(key)) => this.diff_refused(key, cx),
                Err(diff_pane::LoadFailure::Io(error)) => {
                    this.diff_refused_text(io_msg(&error), cx);
                }
            });
        })
        .detach();
    }

    /// Put a refusal on the comparison pane and focus it.
    fn diff_refused(&mut self, key: &'static str, cx: &mut Context<Self>) {
        let text = self.state.lang.tr(key);
        self.diff_refused_text(text, cx);
    }

    /// The same refusal when the message is already built.
    fn diff_refused_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.right.diff.error = Some(text);
        // A refusal replaces whatever the pane was showing, turn list included.
        self.right.diff.turn = None;
        self.show_right_tab(RightTab::Diff, cx);
        cx.notify();
    }

    /// Highlight and show a comparison parsed for a workspace file.
    ///
    /// The language table is the editor's own, so a patch of a `.tsx` is
    /// highlighted as one and the two surfaces cannot disagree.
    fn present_comparison(
        &mut self,
        rel: &Path,
        parsed: Vec<DiffFile>,
        source: DiffSource,
        cx: &mut Context<Self>,
    ) {
        let mut parsed = parsed;
        if let Some(language) = diff_pane::language_for(rel) {
            parsed = parsed
                .into_iter()
                .map(|file| file.with_language(language))
                .collect();
        }
        self.show_comparison(rel.to_path_buf(), parsed, source, cx);
    }

    /// Open a comparison that was built rather than read from a file.
    ///
    /// This is the CHAT side's entry point: a step that wrote a file knows the
    /// change it asked for, and `diff_pane::patch_for` turns that into a patch
    /// the kit parses. The pane is the same one a `.patch` file opens, so a
    /// request and a file are read the same way. The source is always
    /// read-only here, because a request is not a document that can be saved
    /// back over itself.
    pub(crate) fn open_diff_text(&mut self, label: &str, patch: &str, cx: &mut Context<Self>) {
        let files = match DiffFile::parse(patch) {
            Ok(files) if !files.is_empty() => files,
            _ => {
                self.right.diff.error = Some(self.state.lang.tr("diff.error"));
                self.right.diff.turn = None;
                self.show_right_tab(RightTab::Diff, cx);
                cx.notify();
                return;
            }
        };
        let path = PathBuf::from(label);
        let files = match diff_pane::language_for(&path) {
            Some(language) => files
                .into_iter()
                .map(|file| file.with_language(language))
                .collect(),
            None => files,
        };
        self.show_comparison(path, files, DiffSource::Patch, cx);
    }

    /// Put a parsed comparison on screen and focus the pane.
    ///
    /// Every arrival - a file, a conflict, a request - ends here, so the tab,
    /// the resolution subscription and the conflict bookkeeping cannot drift
    /// between the paths that open one.
    fn show_comparison(
        &mut self,
        path: PathBuf,
        parsed: Vec<DiffFile>,
        source: DiffSource,
        cx: &mut Context<Self>,
    ) {
        let conflicts: usize = parsed.iter().map(|file| file.conflicts().len()).sum();
        let mode = diff_pane::mode_of(&self.state.settings.global.general.diff_mode);
        let files = parsed.clone();
        let state = cx.new(|cx| {
            let mut state = DiffState::new(files, cx);
            state.set_mode(mode, cx);
            state
        });
        // The pane has to know when a conflict is resolved, because the save
        // control appears only once nothing is left unresolved. Reading it off
        // the state is what keeps a resolution made by the kit's own commands
        // indistinguishable from one made anywhere else.
        cx.subscribe(&state, |this, state, event: &DiffEvent, cx| {
            if matches!(event, DiffEvent::ConflictResolved(..)) {
                let state = state.read(cx);
                let outstanding: usize = state
                    .files()
                    .iter()
                    .map(|document| {
                        (0..document.conflicts().len())
                            .filter(|ix| state.conflict_resolution(document.path(), *ix).is_none())
                            .count()
                    })
                    .sum();
                this.right.diff.resolved = outstanding == 0;
                cx.notify();
            }
        })
        .detach();
        self.right.diff.state = Some(state);
        self.right.diff.path = Some(path);
        self.right.diff.source = Some(source);
        self.right.diff.error = None;
        // A file comparison and a turn list are two subjects for one surface;
        // opening a file takes the surface back.
        self.right.diff.turn = None;
        self.right.diff.conflicts = conflicts;
        // Nothing is resolved the moment a conflicted file opens; the flag is
        // recomputed on every resolution rather than assumed.
        self.right.diff.resolved = conflicts == 0;
        self.show_right_tab(RightTab::Diff, cx);
        cx.notify();
    }

    /// Set the comparison's view mode, remembering it for the next file.
    ///
    /// The mode is a preference, not a per-file fact: the reference keeps one
    /// choice for the reader, so opening the next patch shows the shape they
    /// last asked for.
    pub(crate) fn set_diff_mode(&mut self, mode: DiffMode, cx: &mut Context<Self>) {
        if let Some(state) = self.right.diff.state.clone() {
            state.update(cx, |state, cx| state.set_mode(mode, cx));
        }
        self.right.diff.mode = mode;
        let key = diff_pane::mode_key(mode);
        if self.state.settings.global.general.diff_mode != key {
            self.state.settings.global.general.diff_mode = key.to_string();
            self.save_settings(cx);
        }
        cx.notify();
    }

    /// Flip between the two views, which is what the strip's control does.
    pub(crate) fn toggle_diff_mode(&mut self, cx: &mut Context<Self>) {
        let mode = match self.right.diff.mode {
            DiffMode::Split => DiffMode::Unified,
            _ => DiffMode::Split,
        };
        self.set_diff_mode(mode, cx);
    }

    /// Whether long lines wrap, remembering it like the mode.
    pub(crate) fn toggle_diff_wrap(&mut self, cx: &mut Context<Self>) {
        let wrap = !self.state.settings.global.general.diff_wrap;
        self.state.settings.global.general.diff_wrap = wrap;
        self.save_settings(cx);
        cx.notify();
    }

    /// Write the merged text back over the conflicted file.
    ///
    /// Every conflict must be resolved first: a partially merged file is not a
    /// merge, and writing one would leave markers on disk with the pane saying
    /// it was done.
    pub(crate) fn save_merged(&mut self, cx: &mut Context<Self>) {
        let (Some(state), Some(rel)) =
            (self.right.diff.state.clone(), self.right.diff.path.clone())
        else {
            return;
        };
        let Some(root) = self.state.workspace_root() else {
            return;
        };
        let label = rel.to_string_lossy().to_string();
        let Some(text) = state.read(cx).resolved_text(&label) else {
            self.right.diff.error = Some(self.state.lang.tr("diff.unresolved"));
            cx.notify();
            return;
        };
        match files::write(&root, &rel, &text) {
            Ok(()) => {
                // The editor may hold the same file with the markers still in
                // its buffer, so the tab is replaced rather than left stale.
                self.files.tabs.retain(|tab| tab.rel != rel);
                self.right.diff.resolved = false;
                self.right.diff.conflicts = 0;
                self.right.diff.source = None;
                self.right.diff.error = None;
                // Re-read it: the merged file has no conflicts left, so it comes
                // back as its own single column, which is the proof it landed.
                self.open_diff(&rel, cx);
            }
            Err(error) => {
                self.right.diff.error = Some(io_msg(&error));
                cx.notify();
            }
        }
    }

    /// The comparison surface.
    pub(crate) fn diff_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        // A turn-focused list is the pane's other subject. It is checked first
        // because the two are mutually exclusive and a turn has no kit
        // `DiffState` behind it - the list is built from the tool records.
        if self.right.diff.turn.is_some() {
            return self.turn_diff_pane(cx);
        }
        let lang = self.state.lang;
        let pane = div()
            .debug_selector(|| String::from("diff-pane"))
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .overflow_hidden();
        let Some(state) = self.right.diff.state.clone() else {
            // No comparison yet: name the three ways one arrives rather than
            // drawing an empty box.
            return pane
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(6.0))
                        .px(px(theme::RIGHT_PAD))
                        .child(icon(IconName::GitCompareArrows, 22.0, theme::faint()))
                        .child(txt(lang.tr("diff.empty"), theme::CHIP_SIZE, theme::text()))
                        // Name the two ways a comparison arrives. A pane that
                        // only says it is empty is the reason this surface read
                        // as unreachable: the reader had no way to know the tab
                        // needs a FILE, not a setting.
                        .child(
                            div()
                                .debug_selector(|| String::from("diff-empty-hint"))
                                .px(px(20.0))
                                .child(
                                    txt(
                                        lang.tr("diff.emptyHint"),
                                        theme::CHIP_SIZE,
                                        theme::faint(),
                                    )
                                    .line_height(px(17.0)),
                                ),
                        ),
                )
                .children(self.right.diff.error.clone().map(|error| {
                    txt(error, theme::CHIP_SIZE, theme::error())
                        .debug_selector(|| String::from("diff-error"))
                }))
                .into_any_element();
        };
        let wrap = self.state.settings.global.general.diff_wrap;
        let mode = self.right.diff.mode;
        let name = self
            .right
            .diff
            .path
            .as_ref()
            .map(|path| diff_pane::display_name(path))
            .unwrap_or_default();
        // The counts are the pane's own title detail: a file row already says
        // the path, so the header spends the space on what changed.
        let (added, deleted) = {
            let document = state.read(cx);
            document
                .files()
                .iter()
                .fold((0usize, 0usize), |(a, d), file| {
                    (a + file.additions(), d + file.deletions())
                })
        };
        // A merge can be written back, and only once nothing is left
        // unresolved: a partially merged file is not a merge. The predicate is
        // `DiffView`'s own so the control and the handler cannot disagree.
        let mergeable = self.right.diff.is_merge();
        let chip = |id: &'static str, label: String, on: bool| {
            let hover = label.clone();
            div()
                .debug_selector(move || id.to_string())
                .id(SharedString::from(id))
                .h(px(20.0))
                .px(px(7.0))
                .flex()
                .items_center()
                .flex_none()
                .rounded(px(6.0))
                .cursor_pointer()
                .when(on, |cell| cell.bg(rgb(theme::hover())))
                .child(if on {
                    txt(label, 11.0, theme::text()).into_any_element()
                } else {
                    txt(hover.clone(), 11.0, theme::faint()).into_any_element()
                })
        };
        let header = div()
            .debug_selector(|| String::from("diff-toolbar"))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .h(px(32.0))
            .px(px(theme::RIGHT_PAD))
            .flex_none()
            .border_b_1()
            .border_color(rgb(theme::border_soft()))
            .child(icon(IconName::GitCompareArrows, 13.0, theme::faint()))
            .child(
                div()
                    .debug_selector(|| String::from("diff-title"))
                    .flex_1()
                    .min_w_0()
                    .child(txt(name, 12.0, theme::text()).truncate()),
            )
            .when(added > 0, |row| {
                row.child(txt(format!("+{added}"), 11.0, theme::success()))
            })
            .when(deleted > 0, |row| {
                row.child(txt(format!("\u{2212}{deleted}"), 11.0, theme::error()))
            })
            .child(
                chip(
                    "diff-mode",
                    match mode {
                        DiffMode::Split => self.state.lang.tr("diff.modeSplit"),
                        _ => self.state.lang.tr("diff.modeUnified"),
                    },
                    true,
                )
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.toggle_diff_mode(cx);
                })),
            )
            .child(
                chip("diff-wrap", self.state.lang.tr("diff.wrap"), wrap).on_click(cx.listener(
                    |this, _event, _window, cx| {
                        this.toggle_diff_wrap(cx);
                    },
                )),
            )
            .when(mergeable, |row| {
                row.child(
                    chip("diff-save", self.state.lang.tr("diff.save"), true).on_click(cx.listener(
                        |this, _event, _window, cx| {
                            this.save_merged(cx);
                        },
                    )),
                )
            })
            .when(self.right.diff.is_open(), |row| {
                row.child(
                    div()
                        .debug_selector(|| String::from("diff-close"))
                        .id("diff-close")
                        .h(px(20.0))
                        .w(px(20.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_none()
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .hover(|cell| cell.bg(rgb(theme::hover())))
                        .child(icon(IconName::Close, 11.0, theme::faint()))
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.right.diff.clear();
                            this.right.diff.error = None;
                            cx.notify();
                        })),
                )
            });
        let mut body = div()
            .debug_selector(|| String::from("diff-body"))
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .px(px(theme::RIGHT_PAD))
            .py(px(6.0))
            .child(Diff::new(&state).soft_wrap(wrap).h_full().w_full());
        if let Some(error) = self.right.diff.error.clone() {
            body = body.child(txt(error, theme::CHIP_SIZE, theme::error()));
        }
        pane.child(header).child(body).into_any_element()
    }

    /// Open the Diff pane on one turn's changed files.
    ///
    /// The data is the turn's own tool records: the same grouping the closing
    /// card draws, plus the one fact the card hides - whether a file's old side
    /// was recorded. Nothing here reads the disk or a VCS, because a tool
    /// request is the only before-image bsh keeps.
    pub(crate) fn open_diff_turn(&mut self, message: usize, cx: &mut Context<Self>) {
        let Some(session) = self.state.active_session() else {
            return;
        };
        let Some(target) = session.messages.get(message) else {
            return;
        };
        // The tools are cloned out before the shell is written: the session is
        // borrowed from the shell, and the diff view is a field of it.
        let tools = target.tools.clone();
        // The turn's number, one-based: the number of prompts up to and
        // including the message the reader pressed. It names the turn in the
        // header and in the empty state, so a reader landing on a list knows
        // which one they are looking at.
        let ordinal = session
            .messages
            .iter()
            .take(message + 1)
            .filter(|line| line.role == Role::User && !line.steering)
            .count()
            .max(1);
        let files = diff_pane::turn_files(&tools);
        let commands = diff_pane::TurnView::ran_commands(&tools);
        let empty = files
            .is_empty()
            .then(|| diff_pane::TurnView::why_empty(&tools));
        // The file comparison and the turn list are exclusive subjects: the
        // turn takes the surface over, and its toolbar carries no mode or wrap
        // control because neither applies to a recorded hunk.
        self.right.diff.state = None;
        self.right.diff.path = None;
        self.right.diff.source = None;
        self.right.diff.conflicts = 0;
        self.right.diff.resolved = false;
        self.right.diff.turn = Some(diff_pane::TurnView {
            message,
            ordinal,
            files,
            open: std::collections::HashSet::new(),
            rows: std::collections::HashMap::new(),
            empty,
            commands,
        });
        self.right.diff.error = None;
        self.show_right_tab(RightTab::Diff, cx);
        cx.notify();
    }

    /// Open the turn's list with one changed file already expanded.
    ///
    /// A changed-file row names one file, so it lands on that file's comparison
    /// rather than on a list the reader then has to search.
    pub(crate) fn open_diff_turn_at(&mut self, message: usize, path: &str, cx: &mut Context<Self>) {
        self.open_diff_turn(message, cx);
        // The path is the card's own identity for a changed file; an index
        // would assume the two groupings never differ.
        let index = self
            .right
            .diff
            .turn
            .as_ref()
            .and_then(|turn| turn.files.iter().position(|file| file.path == path));
        if let Some(index) = index {
            self.expand_turn_file(index, cx);
        }
        cx.notify();
    }

    /// Open or close one file's rows in the turn list.
    pub(crate) fn toggle_turn_file(&mut self, index: usize, cx: &mut Context<Self>) {
        let open = self
            .right
            .diff
            .turn
            .as_ref()
            .is_some_and(|turn| turn.open.contains(&index));
        if open {
            if let Some(turn) = self.right.diff.turn.as_mut() {
                turn.open.remove(&index);
            }
            cx.notify();
            return;
        }
        self.expand_turn_file(index, cx);
    }

    /// Resolve one file's rows and mark it open.
    ///
    /// The rows are resolved HERE rather than per frame because placing the
    /// hunk reads the file as it is now; the crate's own rule forbids that on
    /// the render path. A file with no recorded old side resolves to none, and
    /// the row shows its basis note instead.
    fn expand_turn_file(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(file) = self
            .right
            .diff
            .turn
            .as_ref()
            .and_then(|turn| turn.files.get(index))
            .cloned()
        else {
            return;
        };
        let current = self.current_text_of(&file.path);
        let rows = diff_pane::turn_rows(&file, current.as_deref());
        if let Some(turn) = self.right.diff.turn.as_mut() {
            if let Some(rows) = rows {
                turn.rows.insert(index, rows);
            }
            turn.open.insert(index);
        }
        cx.notify();
    }

    /// The turn-focused body: a header naming the turn, then one row per file
    /// it wrote, each expandable to the comparison the tool recorded.
    fn turn_diff_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let turn = self
            .right
            .diff
            .turn
            .as_ref()
            .expect("the caller checked the turn view");
        let added: usize = turn.files.iter().map(|file| file.added).sum();
        let deleted: usize = turn.files.iter().map(|file| file.deleted).sum();
        let title = crate::i18n::tr_args(
            lang,
            "diff.turnTitle",
            &[("turn", turn.ordinal.to_string())],
        );
        let header = div()
            .debug_selector(|| String::from("diff-turn-toolbar"))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .h(px(32.0))
            .px(px(theme::RIGHT_PAD))
            .flex_none()
            .border_b_1()
            .border_color(rgb(theme::border_soft()))
            .child(icon(IconName::GitCompareArrows, 13.0, theme::faint()))
            .child(
                div()
                    .debug_selector(|| String::from("diff-turn-title"))
                    .flex_1()
                    .min_w_0()
                    .child(txt(title, 12.0, theme::text()).truncate()),
            )
            .when(added > 0, |row| {
                row.child(txt(format!("+{added}"), 11.0, theme::success()))
            })
            .when(deleted > 0, |row| {
                row.child(txt(format!("\u{2212}{deleted}"), 11.0, theme::error()))
            })
            .child(
                div()
                    .debug_selector(|| String::from("diff-turn-close"))
                    .id("diff-turn-close")
                    .h(px(20.0))
                    .w(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_none()
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(|cell| cell.bg(rgb(theme::hover())))
                    .child(icon(IconName::Close, 11.0, theme::faint()))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.right.diff.clear();
                        this.right.diff.error = None;
                        cx.notify();
                    })),
            );
        // One scroller per turn: a reader who opens another turn's list lands
        // at its top rather than at the previous list's offset.
        let message = turn.message;
        let mut body = div()
            .debug_selector(|| String::from("diff-turn-body"))
            .id(SharedString::from(format!("diff-turn-body-{message}")))
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .overflow_y_scroll();
        if turn.files.is_empty() {
            // An empty list has a reason, and the two reasons are different
            // facts: a turn that wrote nothing, versus one whose writes came
            // from a command nobody recorded file by file.
            let key = match turn.empty.unwrap_or(diff_pane::TurnEmpty::NoWrites) {
                diff_pane::TurnEmpty::CommandsOnly => "diff.turnCommands",
                diff_pane::TurnEmpty::NoWrites => "diff.turnEmpty",
            };
            let text = crate::i18n::tr_args(lang, key, &[("turn", turn.ordinal.to_string())]);
            body = body.child(
                div()
                    .debug_selector(|| String::from("diff-turn-empty"))
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(6.0))
                    .px(px(theme::RIGHT_PAD))
                    .child(icon(IconName::GitCompareArrows, 22.0, theme::faint()))
                    .child(
                        txt(text, theme::CHIP_SIZE, theme::text()).text_align(TextAlign::Center),
                    ),
            );
        } else {
            for (index, file) in turn.files.iter().enumerate() {
                body = body.child(self.turn_file_row(index, file, cx));
            }
            if turn.commands {
                // The list knows the recorded writes. A command in the same
                // turn may have changed more, and saying so is the difference
                // between a summary and a false claim of completeness.
                body = body.child(
                    div()
                        .debug_selector(|| String::from("diff-turn-command-note"))
                        .px(px(theme::RIGHT_PAD))
                        .py(px(8.0))
                        .child(txt(lang.tr("diff.turnCommandNote"), 11.0, theme::faint())),
                );
            }
        }
        div()
            .debug_selector(|| String::from("diff-pane"))
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(header)
            .child(body)
            .into_any_element()
    }

    /// One file in the turn list, with its expandable comparison.
    fn turn_file_row(
        &self,
        index: usize,
        file: &diff_pane::TurnFile,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lang = self.state.lang;
        let turn = self
            .right
            .diff
            .turn
            .as_ref()
            .expect("a file row belongs to a turn");
        let open = turn.open.contains(&index);
        let note = file.basis.note_key().map(|key| lang.tr(key));
        let name = crate::ui::transcript::base_name(&file.path);
        let row = div()
            .debug_selector(move || format!("diff-turn-file-{index}"))
            .id(SharedString::from(format!("diff-turn-file-{index}")))
            .w_full()
            .px(px(theme::RIGHT_PAD))
            .py(px(6.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .hover(|cell| cell.bg(rgb(theme::hover())))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.toggle_turn_file(index, cx);
            }))
            .child(icon(
                if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                },
                12.0,
                theme::faint(),
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(txt(name, 12.0, theme::text()).font_family(theme::CODE_FONT)),
            )
            .when_some(note, |row, note| row.child(txt(note, 11.0, theme::warn())))
            .child(txt(format!("+{}", file.added), 11.0, theme::success()))
            .child(txt(
                format!("\u{2212}{}", file.deleted),
                11.0,
                theme::error(),
            ));
        let mut column = div().w_full().flex().flex_col().child(row);
        if open {
            if let Some(key) = file.basis.note_key() {
                // No before-image was recorded: say so instead of drawing the
                // requested additions as if they were a located hunk.
                column = column.child(
                    div()
                        .debug_selector(move || format!("diff-turn-note-{index}"))
                        .px(px(theme::RIGHT_PAD + 20.0))
                        .py(px(6.0))
                        .child(txt(lang.tr(key), 11.0, theme::warn())),
                );
            } else if let Some(rows) = turn.rows.get(&index) {
                let mut list = div().w_full().flex().flex_col();
                for (line_no, line) in rows.iter().take(diff_pane::TURN_MAX_ROWS).enumerate() {
                    list = list.child(Self::turn_diff_line(index, line_no, line));
                }
                if rows.len() > diff_pane::TURN_MAX_ROWS {
                    let left = rows.len() - diff_pane::TURN_MAX_ROWS;
                    list = list.child(
                        div()
                            .debug_selector(move || format!("diff-turn-more-{index}"))
                            .px(px(theme::RIGHT_PAD + 20.0))
                            .py(px(4.0))
                            .child(txt(
                                crate::i18n::tr_args(
                                    lang,
                                    "diff.turnMore",
                                    &[("count", left.to_string())],
                                ),
                                11.0,
                                theme::faint(),
                            )),
                    );
                }
                column = column.child(list);
            }
        }
        column.into_any_element()
    }

    /// One row of an expanded file: the side's line number, the `-` / `+`
    /// marker, and the line's text.
    fn turn_diff_line(index: usize, line_no: usize, line: &diff_pane::PreviewLine) -> AnyElement {
        let ink = match line.mark {
            '-' | '+' => theme::text(),
            _ => theme::muted(),
        };
        let fill = match line.mark {
            '-' => theme::diff_del(),
            '+' => theme::diff_add(),
            _ => theme::page(),
        };
        let marker = match line.mark {
            '-' => "\u{2212}",
            '+' => "+",
            _ => "",
        };
        let marker_ink = match line.mark {
            '-' => theme::error(),
            '+' => theme::success(),
            _ => theme::faint(),
        };
        let number = line
            .new
            .or(line.old)
            .map(|n| n.to_string())
            .unwrap_or_default();
        div()
            .debug_selector(move || format!("diff-turn-line-{index}-{line_no}"))
            .id(SharedString::from(format!(
                "diff-turn-line-{index}-{line_no}"
            )))
            .w_full()
            .flex()
            .flex_row()
            // The gutter sits on the FIRST visual line of a wrapped row, so it
            // measures from the top rather than centring across the wrap.
            .items_start()
            // A line wider than the pane WRAPS, and the row takes the room the
            // wrap uses. A fixed `h(18)` did not clip it: the second visual
            // line escaped the row and painted over the next row's text, which
            // turned a narrow turn list into overprinted glyphs.
            .min_h(px(18.0))
            .bg(rgb(fill))
            .child(
                txt(number, 11.0, theme::faint())
                    .w(px(34.0))
                    .pr(px(6.0))
                    .flex_none()
                    .line_height(px(18.0))
                    .text_align(TextAlign::Right),
            )
            .child(
                txt(marker, 11.0, marker_ink)
                    .w(px(14.0))
                    .flex_none()
                    .line_height(px(18.0))
                    .text_align(TextAlign::Center),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .px(px(6.0))
                    .child(
                        txt(line.text.clone(), 11.0, ink)
                            .font_family(theme::CODE_FONT)
                            .line_height(px(18.0)),
                    ),
            )
            .into_any_element()
    }
}

/// Whether a captured frame is the page the pane should draw.
///
/// A frame is the reader's page only when the browser holds a document for
/// it. The engine's pre-warm paints `about:blank` before any navigation, and
/// an `Empty` pane must not draw that capture: the empty state (globe plus
/// prompt) is the pane's answer to "nothing loaded yet", and a white capture
/// drawn over it is the same rectangle as a broken pane. Every other status
/// has a document behind the frame, including a failed one, where the frame is
/// the page the reader already had and the failure line sits above it.
pub(crate) fn frame_is_the_page(status: crate::browser::Status) -> bool {
    !matches!(status, crate::browser::Status::Empty)
}

/// Read, change, and save the peer store under the directory lock.
///
/// The UI edits the file only when no federation node is running; the lock
/// still matters because the CLI, another instance, or a second window could
/// be writing at the same time.
pub(crate) fn mutate_store_file<F>(path: &Path, apply: F) -> Result<String, String>
where
    F: FnOnce(&mut federation::PeerStore) -> Result<String, String>,
{
    let directory = path
        .parent()
        .ok_or_else(|| "the peer store has no directory".to_string())?;
    let _lock = crate::federation_lock::DataDirLock::acquire(directory)?;
    let mut store = federation::PeerStore::load(path).unwrap_or_default();
    let notice = apply(&mut store)?;
    store.save(path).map_err(|error| error.to_string())?;
    Ok(notice)
}
/// The pane's status line when no node runs: off is a setting, a failed start
/// is a different fact, and the two must not read the same.
pub(crate) fn federation_status_off(lang: crate::i18n::Lang, enabled: bool) -> String {
    let key = if enabled {
        "right.federation.failed"
    } else {
        "right.federation.off"
    };
    lang.tr(key)
}

/// The colour that stands for one risk class.
#[allow(dead_code)]
fn risk_color(risk: Option<&str>) -> u32 {
    match risk {
        Some("low") => theme::success(),
        Some("medium") => theme::warn(),
        Some("high" | "critical") => theme::error(),
        _ => theme::faint(),
    }
}

/// The expiry preset's label.
#[allow(dead_code)]
fn federation_expiry_label(lang: crate::i18n::Lang, minutes: u64) -> String {
    match minutes {
        0 => lang.tr("right.federation.noExpiry"),
        60 => lang.tr("right.federation.oneHour"),
        1440 => lang.tr("right.federation.oneDay"),
        10080 => lang.tr("right.federation.oneWeek"),
        43200 => lang.tr("right.federation.thirtyDays"),
        other => format!("{other} min"),
    }
}

/// The name a peer shows: the announced one, else the short id.
pub(crate) fn display_name(name: Option<&str>, node: &str) -> String {
    name.map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| short_node(node))
}

/// The short node id the panes show; the full id stays in the store and CLI.
pub(crate) fn short_node(node: &str) -> String {
    node.chars().take(10).collect()
}

/// Render a peer store into the pane's rows, in store order.
pub(crate) fn federation_rows(store: &federation::PeerStore) -> Vec<FederationPeerRow> {
    store
        .iter()
        .map(|record| FederationPeerRow {
            node: record.node.to_string(),
            name: record.note.clone(),
            state: match record.state {
                federation::TrustState::Invited => "invited",
                federation::TrustState::Pending => "pending",
                federation::TrustState::Trusted => "trusted",
                federation::TrustState::Suspended => "suspended",
                federation::TrustState::Revoked => "revoked",
            }
            .to_string(),
            grants: record
                .grants
                .iter()
                .map(|grant| grant.capability.to_string())
                .collect(),
        })
        .collect()
}

/// The newest delegation records, for the pane's read-only list.
pub(crate) fn federation_delegation_rows(
    store: &federation::DelegationStore,
) -> Vec<FederationDelegationRow> {
    let mut records: Vec<_> = store.iter().collect();
    records.sort_by_key(|record| std::cmp::Reverse(record.updated_at_ms));
    records
        .into_iter()
        .take(8)
        .map(|record| FederationDelegationRow {
            id: record.request.id.to_string(),
            target: record.request.target.to_string(),
            objective: record.request.objective.clone(),
            state: match record.state {
                federation::DelegationState::Created => "right.federation.stateCreated",
                federation::DelegationState::Accepted => "right.federation.stateAccepted",
                federation::DelegationState::Running => "right.federation.stateRunning",
                federation::DelegationState::Succeeded => "right.federation.stateSucceeded",
                federation::DelegationState::Failed => "right.federation.stateFailed",
                federation::DelegationState::Cancelled => "right.federation.stateCancelled",
                federation::DelegationState::Rejected => "right.federation.stateRejected",
            },
            error: record.error.clone(),
            cancellable: matches!(
                record.state,
                federation::DelegationState::Created
                    | federation::DelegationState::Accepted
                    | federation::DelegationState::Running
            ),
        })
        .collect()
}

/// The graph status for a federation delegation's state key.
fn peer_graph_status(state: &str) -> crate::ui::task_graph::GraphStatus {
    use crate::ui::task_graph::GraphStatus;
    match state {
        "right.federation.stateRunning" => GraphStatus::Running,
        "right.federation.stateSucceeded" => GraphStatus::Done,
        "right.federation.stateFailed" | "right.federation.stateRejected" => GraphStatus::Failed,
        "right.federation.stateCancelled" => GraphStatus::Cancelled,
        _ => GraphStatus::Queued,
    }
}

/// Files in the local artifact store, sorted by name.
pub(crate) fn federation_artifact_rows(root: &Path) -> Vec<FederationArtifactRow> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut rows: Vec<FederationArtifactRow> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let metadata = entry.metadata().ok()?;
            metadata.is_file().then(|| FederationArtifactRow {
                name: entry.file_name().to_string_lossy().into_owned(),
                bytes: metadata.len(),
                path,
            })
        })
        .collect();
    rows.sort_by(|left, right| left.name.cmp(&right.name));
    rows
}

/// Resource leases this Node granted, sorted by id.
pub(crate) fn federation_lease_rows(store: &federation::LeaseStore) -> Vec<FederationLeaseRow> {
    store
        .iter()
        .map(|lease| FederationLeaseRow {
            id: lease.id.to_string(),
            capability: lease.capability.to_string(),
            holder: lease.holder.to_string(),
            used: lease.used_bytes,
            max: lease.max_bytes,
            expires_at_ms: lease.expires_at_ms,
        })
        .collect()
}

/// Parse a peer id the way the CLI does: it must look like a Node's key.
pub(crate) fn parse_peer(raw: &str) -> Result<agent::capability::NodeId, String> {
    let raw = raw.trim();
    let node = agent::capability::NodeId::new(raw).map_err(|error| error.to_string())?;
    if !federation::identity::is_valid_node_id(&node) {
        return Err("a peer id is the base64 public key a node prints as its id".to_string());
    }
    Ok(node)
}

/// Enroll one peer from a raw node id, for the store operation's tests.
#[cfg(test)]
pub(crate) fn enroll_peer(store: &mut federation::PeerStore, raw: &str) -> Result<String, String> {
    let node = parse_peer(raw)?;
    store.enroll(node.clone(), agent::capability::unix_ms(), None);
    Ok(format!("enrolled {}", short_node(node.as_str())))
}

/// Remove every peer currently in the revoked state, returning how many went.
///
/// The rows are collected before mutating the store because the iterator borrows
/// it. A successful call means the reader asked for exactly the set that was
/// revoked at that moment; a re-announcement of one of those peers starts a new
/// relationship under Discovered.
pub(crate) fn remove_revoked_peers(store: &mut federation::PeerStore) -> Result<usize, String> {
    let nodes: Vec<agent::capability::NodeId> = store
        .iter()
        .filter(|record| record.state == federation::TrustState::Revoked)
        .map(|record| record.node.clone())
        .collect();
    let mut removed = 0;
    for node in nodes {
        if store.remove(&node) {
            removed += 1;
        }
    }
    if removed == 0 {
        return Err("no revoked peers".to_string());
    }
    Ok(removed)
}

/// Remove every peer record, trusted included, returning how many went.
///
/// This is the administrative cleanup: a trusted peer is revoked implicitly
/// by forgetting its record, exactly as the single-row Remove does.
pub(crate) fn remove_all_peers(store: &mut federation::PeerStore) -> Result<usize, String> {
    let nodes: Vec<agent::capability::NodeId> =
        store.iter().map(|record| record.node.clone()).collect();
    let mut removed = 0;
    for node in nodes {
        if store.remove(&node) {
            removed += 1;
        }
    }
    if removed == 0 {
        return Err("no peers to remove".to_string());
    }
    Ok(removed)
}

/// Grant one capability to one peer, for the store operation's tests.
#[cfg(test)]
pub(crate) fn grant_capability(
    store: &mut federation::PeerStore,
    node: &str,
    raw: &str,
    expires_at_ms: Option<u64>,
) -> Result<String, String> {
    let node = parse_peer(node)?;
    let capability: agent::capability::CapabilityId = raw
        .trim()
        .parse()
        .map_err(|error| format!("invalid capability id: {error}"))?;
    store
        .grant(
            &node,
            capability.clone(),
            agent::capability::unix_ms(),
            expires_at_ms,
        )
        .map_err(|error| error.to_string())?;
    Ok(format!(
        "granted {capability} to {}",
        short_node(node.as_str())
    ))
}

/// Enroll one discovered peer and grant exactly the capabilities chosen.
pub(crate) fn enroll_and_grant(
    store: &mut federation::PeerStore,
    node: &str,
    name: Option<&str>,
    grants: &std::collections::BTreeSet<String>,
) -> Result<String, String> {
    let node_id = parse_peer(node)?;
    let note = name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    store.enroll(node_id.clone(), agent::capability::unix_ms(), note);
    for capability in grants {
        let capability: agent::capability::CapabilityId = capability
            .parse()
            .map_err(|error| format!("invalid capability id: {error}"))?;
        store
            .grant(&node_id, capability, agent::capability::unix_ms(), None)
            .map_err(|error| error.to_string())?;
    }
    Ok(format!(
        "enrolled {} with {} grant(s)",
        display_name(name, node),
        grants.len()
    ))
}

/// Revoke one peer and every grant it holds.
pub(crate) fn revoke_peer_action(
    store: &mut federation::PeerStore,
    node: &str,
    label: &str,
) -> Result<String, String> {
    let node = parse_peer(node)?;
    let shown = display_name(Some(label), node.as_str());
    if !store.revoke(&node) {
        return Err(format!("{shown} is not enrolled"));
    }
    Ok(format!("peer {shown} revoked"))
}

/// Withdraw one capability from one peer.
pub(crate) fn revoke_grant_action(
    store: &mut federation::PeerStore,
    node: &str,
    capability: &str,
) -> Result<String, String> {
    let node = parse_peer(node)?;
    let capability: agent::capability::CapabilityId = capability
        .parse()
        .map_err(|error| format!("invalid capability id: {error}"))?;
    if !store.revoke_grant(&node, &capability) {
        return Err(format!("the peer does not hold {capability}"));
    }
    Ok(format!("withdrew {capability}"))
}
/// Build the image the pane draws from one captured frame.
///
/// The buffer was decoded AND channel-swapped on the engine's pump thread, so
/// this moves ready BGRA bytes into the GPUI image; it does not decode
/// anything or copy a viewport on the render path.
pub(crate) fn render_frame(frame: crate::browser::engine::Frame) -> Option<gpui_kit::RenderImage> {
    let pixels = std::sync::Arc::try_unwrap(frame.bgra).unwrap_or_else(|shared| (*shared).clone());
    let buffer = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_raw(
        frame.width,
        frame.height,
        pixels,
    )?;
    Some(gpui_kit::RenderImage::new([image::Frame::new(buffer)]))
}

/// How one terminal's shell is confined, at spawn time or now.
///
/// Three classes, because they are reversed by three different controls: a
/// policy release (the sandbox switch or a preset), the full-access permission
/// release, and confinement itself. Comparing the classes catches a permission
/// change while a shell runs, which the spawn-time policy id alone does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalConfinement {
    /// A policy turned the sandbox off for this shell.
    PolicyReleased,
    /// The full-access permission released it (no preset asked for heel).
    PermissionReleased,
    /// Heel applies, whether or not this backend can host the pane's tty.
    Confined,
}

/// The class recorded by the facts a terminal kept at spawn.
fn terminal_confinement(
    off: Option<crate::terminal::SandboxOff>,
    label: Option<&'static str>,
) -> TerminalConfinement {
    if off.is_some() {
        TerminalConfinement::PolicyReleased
    } else if label == Some("full-access") {
        TerminalConfinement::PermissionReleased
    } else {
        TerminalConfinement::Confined
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{
        point, px, size, Bounds, Keystroke, Modifiers, MouseButton, Pixels, ScrollDelta,
    };

    use super::{
        anchor_line, chord_modifiers, file_tab_floor_with, file_tab_w, file_tab_w_with, named_key,
        page_point, page_wheel, pane_tab_floor_with, pane_tab_w, pane_tab_w_with, plan_open_strip,
        plan_tab_strip, typed_text, BrowserClick, RightTab, StripSlot, BROWSER_WHEEL_GAIN,
        STRIP_GAP, STRIP_OVERFLOW_W, STRIP_TAB_MIN_W,
    };

    fn key(key: &str, key_char: Option<&str>) -> Keystroke {
        Keystroke {
            key: key.to_string(),
            key_char: key_char.map(str::to_string),
            modifiers: Modifiers::default(),
        }
    }

    #[test]
    fn the_anchor_line_is_the_middle_of_the_part_in_view() {
        let text = "one\ntwo\nthree\nfour\nfive\nsix\n";
        // Lines two through four: "two" up to the start of "five".
        let range = text.find("two").unwrap()..text.find("five").unwrap();
        // Everything laid out: the middle of lines 2..=4.
        assert_eq!(anchor_line(text, &range, &(0..10)), Some(3));
        // The first line scrolled above the viewport; the middle is unchanged.
        assert_eq!(anchor_line(text, &range, &(1..10)), Some(3));
        // Only the tail is laid out: the middle of what is left is line 4.
        assert_eq!(anchor_line(text, &range, &(3..10)), Some(4));
        // The whole selection scrolled past: no line to ride.
        assert_eq!(anchor_line(text, &range, &(5..10)), None);
    }

    #[test]
    fn an_empty_selection_has_no_anchor() {
        assert_eq!(anchor_line("one\ntwo\n", &(0..0), &(0..10)), None);
        assert_eq!(anchor_line("one\ntwo\n", &(3..3), &(0..10)), None);
    }

    #[test]
    fn the_open_strip_spends_its_room_before_truncating() {
        let pane = |label: &str| {
            (
                pane_tab_w_with(label, false),
                pane_tab_floor_with(label, false),
            )
        };
        let panes = [
            (RightTab::Files, pane("Files").0, pane("Files").1),
            (
                RightTab::Extensions,
                pane("Extensions").0,
                pane("Extensions").1,
            ),
            (RightTab::Tasks, pane("Tasks").0, pane("Tasks").1),
        ];
        let file = (
            file_tab_w_with("ARCHITECTURE.md", true),
            file_tab_floor_with("ARCHITECTURE.md", true),
        );
        let files = [file];
        let active = Some(StripSlot::File(0));
        let plan = plan_open_strip(400.0, None, &panes, &files, active);
        assert!(!plan.overflow);
        let used: f32 = plan.visible.iter().map(|(_, width)| *width).sum::<f32>()
            + STRIP_GAP * plan.visible.len().saturating_sub(1) as f32;
        assert!(
            (used - 400.0).abs() < 1.0,
            "the strip uses the pane's width before clipping: {used}"
        );
        let width_of = |slot: StripSlot| {
            plan.visible
                .iter()
                .find(|(candidate, _)| *candidate == slot)
                .map(|(_, width)| *width)
                .unwrap()
        };
        let active_w = width_of(StripSlot::File(0));
        let inactive_w = width_of(StripSlot::Pane(RightTab::Files));
        assert!(
            active_w > inactive_w + 20.0,
            "the selected tab gets the label room: {active_w} vs {inactive_w}"
        );
    }

    /// Every pane that is not the open one, in the order the strip offers them.
    fn launchers() -> [RightTab; 6] {
        [
            RightTab::Diff,
            RightTab::Terminal,
            RightTab::Browser,
            RightTab::Extensions,
            RightTab::Federation,
            RightTab::Tasks,
        ]
    }

    /// Room to spare draws the pane, its documents, and every launcher chip,
    /// with no catalog trigger.
    #[test]
    fn a_wide_strip_keeps_every_tab_and_chip() {
        let files = [file_tab_w("README.md"), file_tab_w("Cargo.toml")];
        let plan = plan_tab_strip(
            900.0,
            Some(RightTab::Files),
            pane_tab_w("Files"),
            &files,
            Some(0),
            &launchers(),
        );
        assert!(
            !plan.overflow,
            "everything fits, so nothing needs a catalog"
        );
        assert_eq!(plan.visible.len(), 1 + files.len() + launchers().len());
        assert_eq!(plan.visible[0].0, StripSlot::Pane(RightTab::Files));
        assert_eq!(plan.visible[1].0, StripSlot::File(0));
        assert_eq!(plan.visible[2].0, StripSlot::File(1));
    }

    /// The Start screen is a tab too: closing the pane leaves its documents
    /// beside the Start chip rather than hiding them with the pane.
    #[test]
    fn the_start_screen_keeps_its_documents_beside_it() {
        let files = [file_tab_w("README.md"), file_tab_w("Cargo.toml")];
        let plan = plan_tab_strip(365.0, None, pane_tab_w("Start"), &files, Some(0), &[]);
        assert_eq!(plan.visible[0].0, StripSlot::Start);
        assert!(plan
            .visible
            .iter()
            .any(|(slot, _)| *slot == StripSlot::File(0)));
        assert!(plan
            .visible
            .iter()
            .any(|(slot, _)| *slot == StripSlot::File(1)));
        assert!(!plan.overflow, "both documents fit beside the Start tab");
    }

    /// A crowded Start strip still holds the document that is open, even though
    /// the Start tab comes first.
    #[test]
    fn a_crowded_start_strip_keeps_the_open_document() {
        let files = [120.0; 8];
        let plan = plan_tab_strip(280.0, None, pane_tab_w("Start"), &files, Some(6), &[]);
        assert!(plan.overflow);
        assert_eq!(plan.visible[0].0, StripSlot::Start);
        assert!(
            plan.visible
                .iter()
                .any(|(slot, _)| *slot == StripSlot::File(6)),
            "the open document must stay beside Start: {:?}",
            plan.visible
        );
    }

    /// The documents outrank the launcher chips: at the panel's default width
    /// both open files stay, and the panes that do not fit move into the
    /// catalog.
    #[test]
    fn documents_outrank_the_launcher_chips() {
        let files = [file_tab_w("README.md"), file_tab_w("Cargo.toml")];
        let plan = plan_tab_strip(
            365.0,
            Some(RightTab::Files),
            pane_tab_w("Files"),
            &files,
            Some(0),
            &launchers(),
        );
        assert!(plan.overflow, "the strip cannot hold all six chips too");
        let shown: Vec<StripSlot> = plan.visible.iter().map(|(slot, _)| *slot).collect();
        assert!(shown.contains(&StripSlot::File(0)));
        assert!(shown.contains(&StripSlot::File(1)));
        assert_eq!(
            shown
                .iter()
                .filter(|slot| matches!(slot, StripSlot::Pane(_)))
                .count(),
            1,
            "only the open pane is drawn: {shown:?}"
        );
    }

    /// A crowded strip keeps the tab on screen and never draws a document
    /// narrower than the legibility floor.
    #[test]
    fn a_crowded_strip_keeps_the_open_document() {
        let files = [120.0; 8];
        let plan = plan_tab_strip(
            280.0,
            Some(RightTab::Files),
            pane_tab_w("Files"),
            &files,
            Some(6),
            &launchers(),
        );
        assert!(plan.overflow);
        assert!(
            plan.visible
                .iter()
                .any(|(slot, _)| *slot == StripSlot::File(6)),
            "the open document must stay on screen: {:?}",
            plan.visible
        );
        for (slot, width) in &plan.visible {
            if matches!(slot, StripSlot::File(_)) {
                assert!(
                    *width >= STRIP_TAB_MIN_W,
                    "a document tab shrank below the floor: {slot:?} is {width}px"
                );
            }
        }
    }

    /// The plan is a budget, not a suggestion: what it draws - tabs, gaps and
    /// the catalog trigger - never adds up to more than the strip may use.
    #[test]
    fn no_plan_draws_past_its_budget() {
        let files: Vec<f32> = (0..9)
            .map(|index| file_tab_w(&format!("document-{index}.md")))
            .collect();
        for budget in [206.0, 280.0, 365.0, 456.0, 900.0] {
            let plan = plan_tab_strip(
                budget,
                Some(RightTab::Files),
                pane_tab_w("Files"),
                &files,
                Some(4),
                &launchers(),
            );
            let drawn = plan.visible.iter().map(|(_, width)| *width).sum::<f32>()
                + plan.visible.len().saturating_sub(1) as f32 * STRIP_GAP
                + if plan.overflow {
                    STRIP_OVERFLOW_W + STRIP_GAP
                } else {
                    0.0
                };
            assert!(
                drawn <= budget + 0.01,
                "budget {budget}: drew {drawn}: {plan:?}"
            );
        }
    }

    /// The pre-warm paints `about:blank` before any navigation, so a capture
    /// exists while no document does. Drawing that capture is the blank pane:
    /// the same white rectangle whether the pane is waiting for a URL or
    /// broken, with no globe and no prompt. The frame is the reader's page only
    /// once a navigation has produced a document for it.
    #[test]
    fn an_empty_pane_never_draws_a_capture() {
        use crate::browser::Status;
        assert!(
            !super::frame_is_the_page(Status::Empty),
            "the pre-warm's about:blank capture is not a page"
        );
        // A failed navigation is the exception that proves it: the frame there
        // is the page the reader already had, and the failure line sits above.
        for status in [Status::Loading, Status::Ready, Status::Failed] {
            assert!(super::frame_is_the_page(status), "{status:?}");
        }
    }

    #[test]
    fn enter_is_the_one_gated_key() {
        let enter = key("enter", None);
        assert!(super::terminal_is_enter(&enter));
        // A chord with the platform modifier belongs to the application.
        let mut chord = enter.clone();
        chord.modifiers.platform = true;
        assert!(!super::terminal_is_enter(&chord));
        // A modified Enter is a different key for a terminal program.
        let mut chord = enter;
        chord.modifiers.control = true;
        assert!(!super::terminal_is_enter(&chord));
    }

    #[test]
    fn the_mirror_follows_typing_and_its_kill_chords() {
        use crate::terminal::Edit;
        assert_eq!(
            super::terminal_edit(&key("a", Some("a"))),
            Some(Edit::Text(String::from("a")))
        );
        assert_eq!(
            super::terminal_edit(&key("s", Some("ß"))),
            Some(Edit::Text(String::from("ß")))
        );
        assert_eq!(
            super::terminal_edit(&key("space", None)),
            Some(Edit::Text(String::from(" ")))
        );
        assert_eq!(
            super::terminal_edit(&key("backspace", None)),
            Some(Edit::Backspace)
        );
        // Motion is not editing, and the gate does not see it.
        for name in ["up", "left", "tab", "escape"] {
            assert_eq!(super::terminal_edit(&key(name, None)), None, "{name}");
        }
        let mut kill = key("u", Some("u"));
        kill.modifiers.control = true;
        assert_eq!(super::terminal_edit(&kill), Some(Edit::KillLine));
        let mut word = key("w", Some("w"));
        word.modifiers.control = true;
        assert_eq!(super::terminal_edit(&word), Some(Edit::KillWord));
        // A cmd chord never reaches the shell's line.
        let mut chord = key("k", Some("k"));
        chord.modifiers.platform = true;
        assert_eq!(super::terminal_edit(&chord), None);
    }

    /// The platform's count is used as-is, bounded at three, and a different
    /// button starts a new sequence: the page's `detail` must never borrow a
    /// left click's count for a right press.
    #[test]
    fn a_press_uses_the_platform_count_and_restarts_for_another_button() {
        let first = BrowserClick::press(None, MouseButton::Left, 1);
        assert_eq!(first.count(), 1);
        assert_eq!(
            BrowserClick::press(Some(&first), MouseButton::Left, 2).count(),
            2,
            "a second left press is a double click"
        );
        assert_eq!(
            BrowserClick::press(Some(&first), MouseButton::Left, 3).count(),
            3,
            "a third left press is a triple click"
        );
        assert_eq!(
            BrowserClick::press(Some(&first), MouseButton::Left, 7).count(),
            3,
            "the page's own events only name one, two and three"
        );
        assert_eq!(
            BrowserClick::press(Some(&first), MouseButton::Left, 0).count(),
            1,
            "a platform count of zero is still one press"
        );
        assert_eq!(
            BrowserClick::press(Some(&first), MouseButton::Right, 2).count(),
            1,
            "a right press does not continue the left press's sequence"
        );
    }

    /// The captured release maps an outside point to the page's edge; an
    /// ordinary press outside the frame maps to nothing.
    #[test]
    fn an_outside_release_maps_to_the_page_edge_only_when_clamped() {
        let bounds = Bounds {
            origin: point(px(100.0), px(50.0)),
            size: size(px(200.0), px(100.0)),
        };
        let viewport = (1000, 500);
        assert_eq!(
            page_point(bounds, viewport, point(px(200.0), px(100.0)), false),
            Some((500.0, 250.0)),
            "an inside point maps the same either way"
        );
        assert_eq!(
            page_point(bounds, viewport, point(px(0.0), px(0.0)), false),
            None,
            "a press outside the frame is not the page's"
        );
        assert_eq!(
            page_point(bounds, viewport, point(px(0.0), px(0.0)), true),
            Some((0.0, 0.0))
        );
        let (x, y) = page_point(bounds, viewport, point(px(400.0), px(300.0)), true)
            .expect("an outside point clamps into the frame");
        assert!((x - 995.0).abs() < 0.001, "the last page column: {x}");
        assert!((y - 495.0).abs() < 0.001, "the last page row: {y}");
    }

    fn frame() -> Bounds<Pixels> {
        Bounds {
            origin: point(px(100.0), px(50.0)),
            size: size(px(200.0), px(100.0)),
        }
    }

    /// The window and the page disagree about the wheel's sign: GPUI's delta
    /// moves the CONTENT (positive = the reader scrolls up) while CDP's moves
    /// the VIEWPORT (positive `deltaY` = the page scrolls down). Forwarding the
    /// window's number unchanged scrolls every page backwards.
    #[test]
    fn a_wheel_scrolls_the_page_the_way_the_hand_moved() {
        let up = page_wheel(
            frame(),
            (1000, 500),
            point(px(200.0), px(100.0)),
            &ScrollDelta::Pixels(point(px(0.0), px(50.0))),
            px(20.0),
        )
        .expect("a point inside the frame");
        assert!(
            up.3 < 0.0,
            "a positive window delta scrolls the page up: {up:?}"
        );

        let down = page_wheel(
            frame(),
            (1000, 500),
            point(px(200.0), px(100.0)),
            &ScrollDelta::Pixels(point(px(0.0), px(-50.0))),
            px(20.0),
        )
        .expect("a point inside the frame");
        assert!(
            down.3 > 0.0,
            "a negative window delta scrolls the page down: {down:?}"
        );
        assert_eq!(down.3, -up.3, "the two directions are exact opposites");
    }

    /// A mouse wheel sends whole LINES, one coarse detent per notch, so it is
    /// gained to a comfortable step. A trackpad sends precise pixels that
    /// already carry momentum, so it passes through untouched.
    #[test]
    fn a_wheel_notch_is_gained_and_a_trackpad_delta_is_not() {
        let notch = page_wheel(
            frame(),
            (1000, 500),
            point(px(200.0), px(100.0)),
            &ScrollDelta::Lines(point(0.0, 3.0)),
            px(20.0),
        )
        .expect("a point inside the frame");
        assert_eq!(notch.3, -3.0 * 20.0 * BROWSER_WHEEL_GAIN);
        assert!(
            notch.3.abs() > 3.0 * 20.0,
            "a notch travels further than its raw pixels: {}",
            notch.3
        );

        let precise = page_wheel(
            frame(),
            (1000, 500),
            point(px(200.0), px(100.0)),
            &ScrollDelta::Pixels(point(px(0.0), px(60.0))),
            px(20.0),
        )
        .expect("a point inside the frame");
        assert_eq!(precise.3, -60.0, "a precise delta comes through as itself");
    }

    /// A sideways wheel reaches the page on its own axis; it was hardcoded to
    /// zero before, so a horizontal scroller could not be scrolled at all.
    #[test]
    fn a_horizontal_wheel_reaches_the_page() {
        let (_, _, dx, dy) = page_wheel(
            frame(),
            (1000, 500),
            point(px(200.0), px(100.0)),
            &ScrollDelta::Pixels(point(px(30.0), px(0.0))),
            px(20.0),
        )
        .expect("a point inside the frame");
        assert_eq!(dx, -30.0, "the horizontal delta is negated too");
        assert_eq!(dy, 0.0, "and the axes stay independent");
    }

    /// A wheel delivered at the pane's edge is still a wheel over the page:
    /// CDP would otherwise aim it at nothing and the scroll would be lost.
    #[test]
    fn a_wheel_outside_the_frame_lands_on_the_edge() {
        let (x, y, _, _) = page_wheel(
            frame(),
            (1000, 500),
            point(px(0.0), px(0.0)),
            &ScrollDelta::Lines(point(0.0, 1.0)),
            px(20.0),
        )
        .expect("an outside point clamps into the frame");
        assert_eq!((x, y), (0.0, 0.0));
    }

    /// A named key keeps its modifiers out of its name: the pane has the
    /// physical mask and the engine carries it, so Shift+Arrow extends a
    /// selection instead of arriving as a plain Arrow.
    #[test]
    fn a_named_key_keeps_its_modifiers_out_of_its_name() {
        assert_eq!(named_key("up"), Some("arrowup"));
        assert_eq!(named_key("pagedown"), Some("pagedown"));
        assert_eq!(named_key("a"), None, "a letter is its own text");

        let mut shifted = key("up", None);
        shifted.modifiers.shift = true;
        assert_eq!(
            named_key(&shifted.key),
            Some("arrowup"),
            "the name carries no chord"
        );
        assert_eq!(
            chord_modifiers(&shifted.modifiers),
            crate::browser::engine::cdp::SHIFT,
            "the mask carries it instead"
        );
    }

    /// Option composition is text; an Option accelerator is not.
    #[test]
    fn option_composition_is_typed_but_an_option_accelerator_is_not() {
        assert_eq!(typed_text(&key("a", Some("a"))), Some("a"));

        let mut shifted = key("a", Some("A"));
        shifted.modifiers.shift = true;
        assert_eq!(typed_text(&shifted), Some("A"));

        // macOS Option+a produces "å": the character differs from the key, so
        // this is composition and the page must receive it.
        let mut composed = key("a", Some("å"));
        composed.modifiers.alt = true;
        assert_eq!(typed_text(&composed), Some("å"));

        // Option+f has no composed character here, so it is a menu key.
        let mut accelerator = key("f", Some("f"));
        accelerator.modifiers.alt = true;
        assert_eq!(typed_text(&accelerator), None);

        let mut command = key("s", Some("s"));
        command.modifiers.platform = true;
        assert_eq!(typed_text(&command), None);

        let mut control = key("c", Some("c"));
        control.modifiers.control = true;
        assert_eq!(typed_text(&control), None);

        assert_eq!(typed_text(&key("tab", Some("\t"))), None);
    }
}

#[cfg(test)]
mod federation_pane_tests {
    use super::{
        enroll_peer, federation_rows, grant_capability, remove_all_peers, remove_revoked_peers,
        revoke_grant_action, revoke_peer_action, short_node,
    };
    use federation::{NodeIdentity, PeerStore};

    fn peer() -> String {
        NodeIdentity::from_seed([5u8; 32]).node_id().to_string()
    }

    /// Bulk cleanup removes only the revoked rows, leaving trusted peers alone.
    /// The administrative cleanup forgets every record, trusted included.
    #[test]
    fn bulk_removal_forgets_trusted_and_revoked_peers() {
        let trusted = NodeIdentity::from_seed([61u8; 32]).node_id();
        let revoked = NodeIdentity::from_seed([62u8; 32]).node_id();
        let mut store = PeerStore::new();
        store.enroll(trusted.clone(), 1_000, None);
        store.enroll(revoked.clone(), 1_000, None);
        store.revoke(&revoked);

        assert_eq!(remove_all_peers(&mut store).expect("two peers"), 2);
        assert!(!store.iter().any(|record| record.node == trusted));
        assert!(!store.iter().any(|record| record.node == revoked));
        assert!(remove_all_peers(&mut store).is_err(), "nothing is left");
    }

    #[test]
    fn bulk_removal_forgets_only_revoked_peers() {
        let trusted = NodeIdentity::from_seed([51u8; 32]).node_id();
        let revoked = NodeIdentity::from_seed([52u8; 32]).node_id();
        let mut store = PeerStore::new();
        store.enroll(trusted.clone(), 1_000, None);
        store.enroll(revoked.clone(), 1_000, None);
        store.revoke(&revoked);

        let removed = remove_revoked_peers(&mut store).expect("one revoked peer");
        assert_eq!(removed, 1);
        assert!(store.iter().any(|record| record.node == trusted));
        assert!(!store.iter().any(|record| record.node == revoked));
        assert!(
            remove_revoked_peers(&mut store).is_err(),
            "a second cleanup has nothing to remove"
        );
    }

    #[test]
    fn delegation_rows_are_newest_first_and_carry_state() {
        use federation::{DelegationId, DelegationRequest, DelegationStore, NodeIdentity};

        let target = NodeIdentity::from_seed([41u8; 32]).node_id();
        let mut store = DelegationStore::new();
        for (id, objective, now) in [("d-1", "older", 100), ("d-2", "newer", 200)] {
            store
                .create(
                    DelegationRequest {
                        id: DelegationId::new(id),
                        parent: None,
                        target: target.clone(),
                        objective: objective.to_string(),
                        capabilities: Vec::new(),
                        operations: Vec::new(),
                        resources: Vec::new(),
                        deadline_ms: None,
                        allow_sub_delegation: false,
                        result_contract: None,
                        trace_id: None,
                        span_id: None,
                    },
                    now,
                )
                .expect("created");
        }

        let rows = super::federation_delegation_rows(&store);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "d-2");
        assert_eq!(rows[0].objective, "newer");
        assert_eq!(rows[0].state, "right.federation.stateCreated");
        assert_eq!(rows[0].target, target.to_string());
    }

    #[test]
    fn artifact_rows_list_files_sorted_by_name() {
        let root = std::env::temp_dir().join(format!("bsh-artifacts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("root");
        std::fs::write(root.join("b.bin"), b"12").expect("b");
        std::fs::write(root.join("a.bin"), b"1").expect("a");

        let rows = super::federation_artifact_rows(&root);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "a.bin");
        assert_eq!(rows[1].bytes, 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn expiry_presets_are_labelled_in_words() {
        use super::federation_expiry_label;
        use crate::i18n::Lang;

        assert_eq!(federation_expiry_label(Lang::En, 0), "No expiry");
        assert_eq!(federation_expiry_label(Lang::En, 60), "1 hour");
        assert_eq!(federation_expiry_label(Lang::En, 1440), "24 hours");
        assert_eq!(federation_expiry_label(Lang::En, 10080), "7 days");
        assert_eq!(federation_expiry_label(Lang::En, 43200), "30 days");
        assert_eq!(federation_expiry_label(Lang::En, 45), "45 min");
    }
    #[test]
    fn a_named_peer_shows_its_name_instead_of_the_key() {
        use super::display_name;

        assert_eq!(
            display_name(Some("studio-mac"), "abcdefghijkl"),
            "studio-mac"
        );
        assert_eq!(display_name(None, "abcdefghijkl"), "abcdefghij");
        assert_eq!(
            display_name(Some("   "), "abcdefghijkl"),
            "abcdefghij",
            "an empty name falls back to the id"
        );
    }

    #[test]
    fn short_node_keeps_ten_characters() {
        assert_eq!(short_node("abcdefghijklmnop"), "abcdefghij");
    }

    #[test]
    fn enroll_parses_a_real_node_id_and_rows_show_the_grant() {
        let mut store = PeerStore::default();
        let node = peer();

        let notice = enroll_peer(&mut store, &node).expect("enrolled");
        assert!(notice.contains(&short_node(&node)), "{notice}");
        assert_eq!(federation_rows(&store).len(), 1);

        let notice =
            grant_capability(&mut store, &node, "builtin/read_file", None).expect("granted");
        assert!(notice.contains("builtin/read_file"), "{notice}");
        assert_eq!(
            federation_rows(&store)[0].grants,
            vec!["builtin/read_file".to_string()]
        );
    }

    #[test]
    fn an_invalid_node_id_is_refused_before_it_reaches_the_store() {
        let mut store = PeerStore::default();

        assert!(enroll_peer(&mut store, "not-a-key").is_err());
        assert!(store.iter().next().is_none(), "nothing was enrolled");
    }

    #[test]
    fn an_invalid_capability_is_refused() {
        let mut store = PeerStore::default();
        let node = peer();
        enroll_peer(&mut store, &node).expect("enrolled");

        assert!(grant_capability(&mut store, &node, "not-a-capability", None).is_err());
        assert!(federation_rows(&store)[0].grants.is_empty());
    }

    #[test]
    fn a_grant_can_carry_an_expiry() {
        let mut store = PeerStore::default();
        let node = peer();
        enroll_peer(&mut store, &node).expect("enrolled");
        let expires = agent::capability::unix_ms() + 60_000;

        grant_capability(&mut store, &node, "builtin/read_file", Some(expires)).expect("granted");

        let record = store.iter().next().expect("the peer");
        assert_eq!(record.grants[0].expires_at_ms, Some(expires));
    }

    #[test]
    fn enrolling_a_discovered_peer_grants_only_the_chosen_capabilities() {
        use super::enroll_and_grant;
        use std::collections::BTreeSet;

        let node = peer();
        let grants: BTreeSet<String> =
            ["builtin/read_file".to_string(), "builtin/glob".to_string()]
                .into_iter()
                .collect();
        let mut store = PeerStore::default();

        let notice =
            enroll_and_grant(&mut store, &node, Some("studio"), &grants).expect("enrolled");
        assert!(notice.contains("2 grant"), "{notice}");

        let record = store.iter().next().expect("the peer");
        let granted: BTreeSet<String> = record
            .grants
            .iter()
            .map(|grant| grant.capability.to_string())
            .collect();
        assert_eq!(granted, grants);

        let bad: BTreeSet<String> = ["not-a-capability".to_string()].into_iter().collect();
        assert!(enroll_and_grant(&mut PeerStore::default(), &node, None, &bad).is_err());
    }
    #[test]
    fn the_status_line_tells_off_apart_from_a_failed_start() {
        use super::federation_status_off;
        use crate::i18n::Lang;

        let off = federation_status_off(Lang::En, false);
        assert!(off.contains("off in settings"), "{off}");

        let failed = federation_status_off(Lang::En, true);
        assert!(failed.contains("enabled"), "{failed}");
        assert!(failed.contains("launch log"), "{failed}");
    }
    #[test]
    fn the_file_edit_path_reads_and_writes_under_the_lock() {
        use super::mutate_store_file;

        let root = std::env::temp_dir().join(format!("bsh-peer-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a scratch directory");
        let path = root.join("peers.json");
        let node = peer();

        let notice = mutate_store_file(&path, |store| enroll_peer(store, &node)).expect("enrolled");
        assert!(notice.contains("enrolled"), "{notice}");

        let store = federation::PeerStore::load(&path).expect("the store");
        assert_eq!(store.iter().count(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_second_writer_cannot_edit_the_store_file() {
        use super::mutate_store_file;

        let root = std::env::temp_dir().join(format!("bsh-peer-locked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a scratch directory");
        let path = root.join("peers.json");
        let _held = crate::federation_lock::DataDirLock::acquire(&root).expect("the first writer");

        let refused = mutate_store_file(&path, |_store| Ok("changed".to_string()));

        assert!(
            refused.is_err(),
            "the directory lock refuses a second writer"
        );
        assert!(!path.exists(), "nothing was written");
        let _ = std::fs::remove_dir_all(&root);
    }
    #[test]
    fn revoking_a_grant_then_a_peer_is_reported() {
        let mut store = PeerStore::default();
        let node = peer();
        enroll_peer(&mut store, &node).expect("enrolled");
        grant_capability(&mut store, &node, "builtin/read_file", None).expect("granted");

        revoke_grant_action(&mut store, &node, "builtin/read_file").expect("withdrawn");
        assert!(federation_rows(&store)[0].grants.is_empty());
        assert!(revoke_grant_action(&mut store, &node, "builtin/read_file").is_err());

        // Revocation is a state, not a deletion: the row stays and reports
        // "revoked", so the pane shows what happened.
        revoke_peer_action(&mut store, &node, "studio").expect("revoked");
        let rows = federation_rows(&store);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].state, "revoked");
        assert!(rows[0].grants.is_empty());

        let stranger = federation::NodeIdentity::from_seed([6u8; 32])
            .node_id()
            .to_string();
        assert!(revoke_peer_action(&mut store, &stranger, "stranger").is_err());
    }
}
