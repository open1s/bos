//! The right panel: Workspace Files, and the frame the terminal and browser
//! panes will hang off.
//!
//! The panel is the reference's own right column: a tab strip that carries one
//! chip per open pane, a header of controls on its right edge, and either the
//! Start screen's three cards or the body of the pane the reader chose. Only
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

use gpui_kit::component::input::{Editor, EditorState, Input, InputEvent, InputState};
use gpui_kit::*;

use super::shell::{Shell, SideDrag};
use super::{hairline_left, icon, txt, DivExt, IconName};
use crate::files::{self, Kind};
use crate::theme;

// Save the file the panel is showing, bound to cmd-s.
//
// The editor surface carries no save button - the panel's body is the
// editor, and a bar repeating the tab chip's name and one control read as
// chrome - so the keyboard shortcut is the whole affordance, the way a
// native document editor does it.
actions!(bsh_files, [SaveFile]);

/// The wording the panel reports an I/O failure with.
///
/// The reference prints its own `fileReferences` error text beside the row
/// that failed; bsh has no remote to quote, so it prints what the operating
/// system said, which is the more useful half of the same idea.
fn io_msg(error: &std::io::Error) -> String {
    error.to_string()
}

/// Which pane the right panel is showing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RightTab {
    /// The workspace file browser and editor.
    Files,
    /// The managed terminal. Not built yet; the chip exists so the frame is
    /// the one the later panes land in.
    Terminal,
    /// The builtin browser. Not built yet; see [`RightTab::Terminal`].
    Browser,
}

impl RightTab {
    /// The tab's label key in the string table.
    pub(crate) fn key(self) -> &'static str {
        match self {
            RightTab::Files => "right.tab.files",
            RightTab::Terminal => "right.tab.terminal",
            RightTab::Browser => "right.tab.browser",
        }
    }

    /// The tab's leading glyph.
    pub(crate) fn glyph(self) -> IconName {
        match self {
            RightTab::Files => IconName::FolderOpen,
            RightTab::Terminal => IconName::SquareTerminal,
            RightTab::Browser => IconName::Globe,
        }
    }

    /// Every pane the Start screen offers, in the order it lists them.
    const ALL: [RightTab; 3] = [RightTab::Files, RightTab::Terminal, RightTab::Browser];
}

/// The right panel's own state: whether it is open, which pane is up, and the
/// width the reader dragged out.
#[derive(Default)]
pub(crate) struct RightPanel {
    /// Whether the panel is on screen.
    pub(crate) open: bool,
    /// The pane showing, or none for the Start screen.
    pub(crate) tab: Option<RightTab>,
    /// The width a reader dragged out, before it is committed.
    pub(crate) user_w: Option<f32>,
    /// The resize in flight, if any.
    pub(crate) drag: Option<SideDrag>,
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
    /// The notes the reader has pinned to this workspace's files, loaded from
    /// the workspace's own `.bsh` directory and written back on every change.
    pub(crate) notes: crate::notes::Notes,
    /// Whether the notes drawer is showing over the document.
    pub(crate) notes_open: bool,
    /// The note being typed into the drawer's field.
    pub(crate) note_text: Entity<InputState>,
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
        note_text: Entity<InputState>,
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
            notes: crate::notes::Notes::default(),
            notes_open: false,
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

impl Shell {
    /// The right panel column and its resize strip, absent while it is folded.
    pub(crate) fn render_right_panel(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !self.right.open {
            return None;
        }
        let width = self.right_panel_w();
        let column = div()
            .debug_selector(|| String::from("right-panel"))
            .w(px(width))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(theme::page()))
            // The reference hangs the same `.5px` rule on this seam that it
            // hangs on the sidebar's, so the panel reads as a column of the
            // same family.
            .child(self.right_panel_bar(cx))
            .child(self.right_panel_body(cx));
        Some(
            div()
                .debug_selector(|| String::from("right-panel-frame"))
                .relative()
                .w(px(width))
                .h_full()
                .flex_shrink_0()
                .child(hairline_left(column).border_color(rgb(theme::border_soft())))
                .child(self.render_right_panel_handle(cx)),
        )
    }

    /// The reference's DragHandle for the panel: an 8px col-resize strip
    /// straddling the seam, drawn only while the column is up.
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
            .cursor(CursorStyle::ResizeLeftRight)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    this.begin_right_panel_drag(event.position.x.as_f32(), cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                    this.commit_right_panel_width(event.position.x.as_f32(), cx);
                }),
            )
    }

    /// The panel's bar: the Start screen's title, or the tab strip and the
    /// controls the reference draws on every panel header.
    fn right_panel_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let bar = div()
            .debug_selector(|| String::from("right-panel-bar"))
            .h(px(theme::RIGHT_BAR_H))
            .flex_none()
            .px(px(theme::RIGHT_PAD))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.0));
        match self.right.tab {
            None => bar
                .child(icon(IconName::Compass, 15.0, theme::muted()))
                .child(
                    txt(
                        self.state.lang.tr("right.start"),
                        theme::SIDEBAR_SIZE,
                        theme::text(),
                    )
                    .font_weight(FontWeight::MEDIUM),
                )
                .child(div().flex_1())
                .child(self.right_panel_control("right-maximize", IconName::Maximize2, cx))
                .child(self.right_panel_close(cx)),
            Some(tab) => {
                let mut row = bar.child(self.right_tab_chip(tab, cx));
                // The files open out of the Files pane join the panel's own tab
                // strip, right after its chip: the reference's dock carries
                // every open pane and document in one strip, and a second strip
                // inside the pane would read as two unrelated rows.
                if tab == RightTab::Files {
                    for (index, file_tab) in self.files.tabs.iter().enumerate() {
                        row = row.child(self.files_tab_chip(index, file_tab, cx));
                    }
                }
                row.child(
                    div()
                        .id("right-new-tab")
                        .debug_selector(|| String::from("right-new-tab"))
                        .w(px(26.0))
                        .h(px(26.0))
                        .rounded(px(theme::RADIUS_SM))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(theme::hover())))
                        .child(icon(IconName::Plus, 14.0, theme::faint()))
                        // A new tab lands back on the Start screen, which is
                        // the only place a pane can be picked from.
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.right.tab = None;
                            cx.notify();
                        })),
                )
                .child(div().flex_1())
                .child(self.right_panel_control("right-maximize", IconName::Maximize2, cx))
                // The rightmost control is the reference's own collapse
                // control; a second panel-toggle before it would be the same
                // action wearing two glyphs.
                .child(self.right_panel_close(cx))
            }
        }
    }

    /// The one open tab, with the close that drops the pane back to Start.
    ///
    /// For the Files pane the chip is also the tree's own tab: when a document
    /// holds the body, the chip sits unselected and pressing it hands the body
    /// back, which is how the reference's dock moves between a pane and the
    /// documents opened out of it.
    fn right_tab_chip(&self, tab: RightTab, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        let selected = match tab {
            RightTab::Files => self.files.active.is_none(),
            _ => true,
        };
        div()
            .id("right-tab")
            .debug_selector(|| String::from("right-tab"))
            .h(px(28.0))
            .px(px(8.0))
            .rounded(px(theme::RADIUS_SM))
            .when(selected, |chip| chip.bg(rgb(theme::selected_row())))
            .when(!selected, |chip| chip.cursor_pointer())
            .when(!selected, |chip| chip.hover(|s| s.bg(rgb(theme::hover()))))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .child(icon(
                tab.glyph(),
                14.0,
                if selected {
                    theme::text()
                } else {
                    theme::muted()
                },
            ))
            .child(txt(
                lang.tr(tab.key()),
                theme::CHIP_SIZE,
                if selected {
                    theme::text()
                } else {
                    theme::muted()
                },
            ))
            .child(
                div()
                    .id("right-tab-close")
                    .debug_selector(|| String::from("right-tab-close"))
                    .ml(px(2.0))
                    .px(px(3.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(theme::hover())))
                    .child(icon(IconName::X, 12.0, theme::faint()))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.right.tab = None;
                        cx.notify();
                    })),
            )
            .when(tab == RightTab::Files && !selected, |chip| {
                chip.on_click(cx.listener(|this, _event, _window, cx| {
                    this.focus_tree(cx);
                }))
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
            .rounded(px(theme::RADIUS_SM))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(theme::hover())))
            .child(icon(mark, 14.0, theme::muted()))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                // Neither control has a second meaning yet: the panel toggle
                // folds the column and the maximiser says so, so a reader who
                // presses one sees the column move rather than a no-op.
                this.right.open = false;
                this.right.tab = None;
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
            .rounded(px(theme::RADIUS_SM))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(theme::hover())))
            .child(icon(IconName::PanelRightClose, 14.0, theme::muted()))
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.right.open = false;
                this.right.tab = None;
                cx.notify();
            }))
    }

    /// The panel body: the Start screen, or the chosen pane.
    fn right_panel_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let body = div()
            .debug_selector(|| String::from("right-panel-body"))
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .overflow_hidden();
        match self.right.tab {
            None => body.child(self.right_start(cx)),
            Some(RightTab::Files) => body.child(self.files_pane(cx)),
            Some(RightTab::Terminal) | Some(RightTab::Browser) => {
                body.child(self.right_pane_placeholder(self.right.tab.expect("Some arm")))
            }
        }
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

    /// The Start screen: the panel's headline over its three cards.
    ///
    /// Measured from the reference's own guide: a centred column with a 14px
    /// gap and 24px side padding, and an invisible 10% spacer after the last
    /// card that biases the whole stack upward instead of dead-centring it.
    fn right_start(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut stack = div()
            .id("right-start")
            .debug_selector(|| String::from("right-start"))
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(14.0))
            .px(px(24.0));
        stack = stack.child(
            txt(
                self.state.lang.tr("right.start"),
                theme::SIDEBAR_SIZE,
                theme::muted(),
            )
            .mb(px(16.0)),
        );
        for tab in RightTab::ALL {
            stack = stack.child(self.right_start_card(tab, cx));
        }
        // The reference's `::after { flex: 0 10% }`: a spacer that takes no
        // content but takes space, so the visible stack sits above the optical
        // centre of the panel rather than dead-centred in it.
        stack.child(div().flex_none().h(relative(0.10)))
    }

    /// One Start card: glyph, title group, and the shortcut hint trailing.
    ///
    /// Measured: 380px wide (max-width 100%), min-height 56, 14px/20px
    /// padding, a 14px gap, a `.5px` border at the panel's own radius, and a
    /// 26px icon. The whole card is the target.
    fn right_start_card(&self, tab: RightTab, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        let hint = match tab {
            RightTab::Files => "\u{2318}P",
            RightTab::Terminal => "\u{2325}`",
            RightTab::Browser => "\u{2318}T",
        };
        div()
            .id(SharedString::from(format!("right-card-{}", tab.key())))
            .debug_selector(|| format!("right-card-{}", tab.key()))
            .w(px(theme::RIGHT_CARD_W))
            .max_w(px(theme::RIGHT_CARD_W))
            .min_h(px(theme::RIGHT_CARD_H))
            .px(px(20.0))
            .py(px(14.0))
            .rounded(px(theme::RADIUS_LG))
            .border_1()
            .border_color(rgb(theme::border_soft()))
            .bg(rgb(theme::new_session_fill()))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(theme::hover())))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(14.0))
            .child(
                div()
                    .w(px(26.0))
                    .h(px(26.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(tab.glyph(), 20.0, theme::muted())),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(
                        txt(lang.tr(tab.key()), theme::SIDEBAR_SIZE, theme::text())
                            .line_height(px(20.0))
                            .truncate()
                            .flex_1()
                            .min_w_0(),
                    )
                    .child(
                        txt(
                            lang.tr(match tab {
                                RightTab::Files => "right.files.hint",
                                RightTab::Terminal => "right.terminal.hint",
                                RightTab::Browser => "right.browser.hint",
                            }),
                            theme::RIGHT_HINT_SIZE,
                            theme::faint(),
                        )
                        .line_height(px(15.0))
                        .truncate()
                        .flex_1()
                        .min_w_0(),
                    ),
            )
            .child(
                txt(hint, theme::RIGHT_HINT_SIZE, theme::faint())
                    .font_family("monospace")
                    .flex_none(),
            )
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.open_right_tab(tab, cx);
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

    /// The 38px header bar and the manage strip under it.
    fn files_header(&self, root: &Path, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        let path = root.display().to_string();
        let renaming = self.files.renaming.clone();
        let shown = renaming.is_some();
        // The header itself is the reference's measured 38px bar: path on the
        // left, tools on the right, a .5px rule under it. The manage strip bsh
        // adds sits beneath rather than inside, so the measured bar stays
        // measured and the create/rename fields still have somewhere to live.
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
            // The manage strip: always there and never modal, so a reader who
            // types a name and presses the button gets a file while one who
            // does not is never blocked by a dialog to dismiss.
            .child(
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
                                div().flex_1().min_w_0().child(
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
                                        txt(
                                            lang.tr("right.files.new"),
                                            theme::CHIP_SIZE,
                                            theme::text(),
                                        )
                                        .font_weight(FontWeight::MEDIUM),
                                    )
                                    .on_click(cx.listener(|this, _event, window, cx| {
                                        this.create_file(window, cx);
                                    })),
                            ),
                    )
                    .when(shown, |strip| strip.child(self.files_rename_row(cx))),
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
        list
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
        let rel_rename = rel.to_path_buf();
        let rel_delete = rel.to_path_buf();
        let rel_toggle = rel.to_path_buf();
        let group = SharedString::from(format!("file-{name}"));
        let row_id = SharedString::from(format!("file-row-{name}"));
        let row_group = group.clone();
        let size = row.size;
        let deleting = self.files.deleting.as_deref() == Some(rel);
        let renaming = self.files.renaming.as_deref() == Some(rel);
        let expanded = self.files.expanded.iter().any(|p| p == rel);
        let glyph = file_glyph(row);
        let shell = cx.entity();
        div()
            .debug_selector(|| row_id.to_string())
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
                    .child(
                        div()
                            .id(SharedString::from(format!("rename-{}", row.name)))
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
                            .id(SharedString::from(format!("delete-{}", row.name)))
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
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = self.files.active == Some(index);
        let name = tab.label();
        let chip_name = name.clone();
        let dirty = tab.dirty;
        let active_color = if active {
            theme::text()
        } else {
            theme::muted()
        };
        let weight = if active {
            FontWeight::MEDIUM
        } else {
            FontWeight::default()
        };
        let close_name = name.clone();
        div()
            .debug_selector(move || format!("file-tab-{chip_name}"))
            .id(SharedString::from(format!("file-tab-{index}")))
            .h(px(26.0))
            .px(px(8.0))
            .rounded(px(6.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .cursor_pointer()
            .when(active, |chip| chip.bg(rgb(theme::selected_row())))
            .hover(|s| s.bg(rgb(theme::hover())))
            .child(icon(file_glyph_for_name(&name), 12.0, theme::faint()).flex_none())
            .child(txt(name, theme::CHIP_SIZE, active_color).font_weight(weight))
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
            .child(
                div()
                    .id(SharedString::from(format!("file-tab-close-{index}")))
                    .debug_selector(move || format!("file-tab-close-{close_name}"))
                    .px(px(3.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(theme::hover())))
                    .child(icon(IconName::X, 11.0, theme::faint()))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        cx.stop_propagation();
                        this.close_tab(index, cx);
                    })),
            )
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.focus_tab(index, cx);
            }))
    }

    /// The drawer's toggle, parked at the document's top right and revealed on
    /// hover, which keeps the surface clean without burying the feature.
    fn notes_toggle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("files-notes-toggle")
            .debug_selector(|| String::from("files-notes-toggle"))
            .group("files-notes")
            .absolute()
            .top(px(4.0))
            .right(px(4.0))
            .w(px(26.0))
            .h(px(26.0))
            .rounded(px(theme::RADIUS_SM))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .opacity(0.0)
            .group_hover("files-notes", |s| s.opacity(1.0))
            .hover(|s| s.bg(rgb(theme::hover())))
            .child(icon(
                IconName::MessageSquare,
                14.0,
                if self.files.notes_open {
                    theme::text()
                } else {
                    theme::muted()
                },
            ))
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.toggle_notes(cx);
            }))
    }

    /// The notes drawer: what the reader pinned to the open file, plus the
    /// field that pins another.
    fn files_notes(&self, rel: &Path, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        let notes = self.files.notes.on(rel);
        div()
            .debug_selector(|| String::from("files-notes"))
            .flex_none()
            .w_full()
            .px(px(theme::RIGHT_PAD))
            .pt(px(6.0))
            .pb(px(6.0))
            .border_b_1()
            .border_color(rgb(theme::border_soft()))
            .flex()
            .flex_col()
            .gap(px(6.0))
            // The pin field is always there, so adding a note is one click and
            // one keystroke away rather than a mode the reader has to enter.
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .debug_selector(|| String::from("files-note-field"))
                            .flex_1()
                            .min_w_0()
                            .child(
                                Input::new(&self.files.note_text)
                                    .id("files-note-field")
                                    .appearance(false)
                                    .h(px(28.0)),
                            ),
                    )
                    .child(
                        div()
                            .id("files-note-add")
                            .debug_selector(|| String::from("files-note-add"))
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
                                txt(
                                    lang.tr("right.files.noteAdd"),
                                    theme::CHIP_SIZE,
                                    theme::text(),
                                )
                                .font_weight(FontWeight::MEDIUM),
                            )
                            .on_click(cx.listener(|this, _event, window, cx| {
                                this.add_note(window, cx);
                            })),
                    ),
            )
            .children(notes.iter().enumerate().map(|(index, note)| {
                let anchor = note.anchor();
                let text = note.text.clone();
                let del_rel = rel.to_path_buf();
                let del_anchor = anchor.clone();
                div()
                    .debug_selector(|| format!("files-note-{anchor}"))
                    .id(SharedString::from(format!("files-note-row-{index}")))
                    .w_full()
                    .px(px(10.0))
                    .py(px(5.0))
                    .rounded(px(theme::RADIUS_SM))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .hover(|s| s.bg(rgb(theme::hover())))
                    .child(
                        txt(anchor, theme::CHIP_SIZE, theme::muted())
                            .flex_none()
                            .w(px(44.0)),
                    )
                    .child(
                        txt(text, theme::SIDEBAR_SIZE, theme::text())
                            .truncate()
                            .flex_1()
                            .min_w_0(),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("files-note-del-{index}")))
                            .debug_selector(move || format!("files-note-del-{del_anchor}"))
                            .px(px(4.0))
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(theme::hover())))
                            .child(icon(IconName::Trash, 12.0, theme::faint()))
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.delete_note(&del_rel, index, cx);
                            })),
                    )
            }))
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
            // The drawer's own toggle, revealed on hover at the document's top
            // right, the way a row's actions reveal - the surface stays clean
            // until the reader asks for it.
            .child(self.notes_toggle(cx))
            .child(match (&open.editor, open.kind) {
                (Some(editor), _) => div()
                    .debug_selector(|| String::from("files-editor-body"))
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(
                        div()
                            .size_full()
                            .px(px(theme::RIGHT_PAD))
                            .py(px(6.0))
                            .border_1()
                            .border_color(rgb(theme::border_soft()))
                            .child(Editor::new(editor)),
                    ),
                (None, Kind::Image) => div()
                    .debug_selector(|| String::from("files-image"))
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .child(
                        img(files::resolve(root, &rel).unwrap_or_default())
                            .max_w(relative(1.0))
                            .max_h(relative(1.0)),
                    ),
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
    }
}

impl Shell {
    /// Fold or unfold the panel, the way the header's left control folds the
    /// sidebar. A fold keeps the pane, so reopening lands where the reader was.
    pub(crate) fn toggle_right_panel(&mut self, cx: &mut Context<Self>) {
        self.right.open = !self.right.open;
        cx.notify();
    }

    /// The right panel column width for this frame, resolved from the reader's
    /// dragged preference against the viewport, the same way the sidebar's is.
    pub(crate) fn right_panel_w(&self) -> f32 {
        theme::right_panel_width_at(self.viewport_w, self.right.user_w)
    }

    /// Open a pane from the Start screen or the tab strip.
    ///
    /// Files is the pane that has to work now; the other two take the tab and
    /// say they are not built, which is what tells a reader pressing Terminal
    /// that they did press it.
    pub(crate) fn open_right_tab(&mut self, tab: RightTab, cx: &mut Context<Self>) {
        self.right.open = true;
        self.right.tab = Some(tab);
        if tab == RightTab::Files && self.files.roots.is_empty() && self.files.tabs.is_empty() {
            self.refresh_files(cx);
        }
        cx.notify();
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
    pub(crate) fn drag_right_panel_width(&mut self, pointer_x: f32, _cx: &mut Context<Self>) {
        let Some(drag) = self.right.drag.as_mut() else {
            return;
        };
        let delta = pointer_x - drag.origin_x;
        if delta.abs() > 2.0 {
            drag.moved = true;
        }
        // The strip sits on the panel's left edge, so dragging left widens it.
        let width = drag.base - delta;
        self.right.user_w = Some(
            width
                .round()
                .clamp(theme::RIGHT_PANEL_MIN, theme::RIGHT_PANEL_MAX),
        );
    }

    /// Release the strip: the width stands, unless it was a click.
    pub(crate) fn commit_right_panel_width(&mut self, _pointer_x: f32, cx: &mut Context<Self>) {
        if let Some(drag) = self.right.drag.take() {
            if !drag.moved {
                self.right.user_w = None;
            }
        }
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

    /// Open a file in the editor area as a tab.
    ///
    /// A file already open is focused rather than reopened, so the strip never
    /// carries the same file twice and a buffer with unsaved changes is never
    /// thrown away by clicking its row again. The read is bounded and the
    /// format decides the body: a text file lands in a highlighted editor, an
    /// image lands in an image pane, and anything else says it cannot be shown
    /// rather than dropping bytes into a buffer.
    pub(crate) fn open_file(&mut self, rel: &Path, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.files.tab_index(rel) {
            self.files.active = Some(index);
            // Reopening a tab shows its remarks if it has any; a file with
            // none keeps the surface clean until the reader opens the drawer.
            self.files.notes_open = self.files.notes.has_notes(rel);
            cx.notify();
            return;
        }
        let Some(root) = self.state.workspace_root() else {
            return;
        };
        let Some(path) = files::resolve(&root, rel) else {
            self.files.error = Some(String::from("That path leaves the workspace."));
            cx.notify();
            return;
        };
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
                cx.subscribe_in(&editor, window, move |this, _editor, event, _window, cx| {
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
                    }
                })
                .detach();
                tab.editor = Some(editor);
                self.files.tabs.push(tab);
                self.files.active = Some(self.files.tabs.len() - 1);
                self.files.notes_open = self.files.notes.has_notes(rel);
                self.files.error = None;
                cx.notify();
            }
            Err(error) => {
                self.files.error = Some(io_msg(&error));
                cx.notify();
            }
        }
    }

    /// Focus a tab by index, which is what pressing its chip does.
    pub(crate) fn focus_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.files.tabs.len() {
            self.files.active = Some(index);
            cx.notify();
        }
    }

    /// Show the tree again, which is what pressing the Files chip does.
    pub(crate) fn focus_tree(&mut self, cx: &mut Context<Self>) {
        self.files.active = None;
        cx.notify();
    }

    /// Close a tab and leave the neighbour that was beside it active.
    ///
    /// Closing the last one empties the editor area, so the tree takes the
    /// panel back over rather than leaving a dead header behind.
    pub(crate) fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.files.tabs.len() {
            return;
        }
        self.files.tabs.remove(index);
        match self.files.active {
            // Closing the tab on screen hands the panel back to the tree,
            // which is the surface the Files chip names.
            Some(open) if open == index => self.files.active = None,
            Some(open) if open > index => self.files.active = Some(open - 1),
            _ => {}
        }
        cx.notify();
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
            }
            Err(error) => self.files.error = Some(io_msg(&error)),
        }
        cx.notify();
    }

    /// Show or hide the notes drawer over the document.
    pub(crate) fn toggle_notes(&mut self, cx: &mut Context<Self>) {
        self.files.notes_open = !self.files.notes_open;
        cx.notify();
    }

    /// Pin a note to the document the panel is showing.
    ///
    /// The note anchors to the editor's own cursor line, so a reader can place
    /// a remark exactly where they are looking; a tab with no editor (an
    /// image, or a file that will not open) takes a whole-file note instead.
    pub(crate) fn add_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        let line = self
            .files
            .active_tab()
            .and_then(|tab| tab.editor.clone())
            .map(|editor| {
                let state = editor.read(cx);
                let offset = state.cursor();
                crate::notes::line_at_offset(state.value().as_ref(), offset)
            });
        let Some(root) = self.state.workspace_root() else {
            return;
        };
        self.files
            .notes
            .add(&rel, crate::notes::Note { line, text });
        if let Err(error) = self.files.notes.save(&root) {
            self.files.error = Some(io_msg(&error));
        }
        self.files.note_text.update(cx, |state, cx| {
            state.set_value(String::new(), window, cx);
        });
        self.files.notes_open = true;
        cx.notify();
    }

    /// Remove one note from the document, by its index in the drawer.
    pub(crate) fn delete_note(&mut self, rel: &Path, index: usize, cx: &mut Context<Self>) {
        let Some(root) = self.state.workspace_root() else {
            return;
        };
        self.files.notes.remove(rel, index);
        if let Err(error) = self.files.notes.save(&root) {
            self.files.error = Some(io_msg(&error));
        }
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
                self.refresh_branch(Path::new(""), cx);
            }
            Err(error) => {
                self.files.error = Some(io_msg(&error));
                cx.notify();
            }
        }
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
        });
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
