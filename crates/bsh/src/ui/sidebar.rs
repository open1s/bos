//! The sidebar: brand, New Session, global panels, the workspace tree, account.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::*;

use super::shell::Shell;
use super::{hairline_all, icon, txt, DivExt, IconName};
use crate::state::age;
use crate::theme;

/// The scroll a navigation key asks for, or `None` when the key is not one.
///
/// A page keeps one row of context on screen, which is what every list does;
/// Home and End are open-ended because the caller clamps them against the real
/// bounds.
pub(crate) fn tree_key_delta(key: &str, page: Pixels) -> Option<f32> {
    let page = f32::from(page) - theme::ROW_H;
    match key {
        "up" => Some(theme::ROW_H),
        "down" => Some(-theme::ROW_H),
        "pageup" => Some(page),
        "pagedown" => Some(-page),
        "home" => Some(f32::INFINITY),
        "end" => Some(f32::NEG_INFINITY),
        _ => None,
    }
}

/// The brand emblem's box in the sidebar row.
///
/// The old harness glyph was 20 wide; the mantis-shrimp emblem carries its own
/// padding inside the square, so it takes 24 to read at the same weight beside
/// the 18px wordmark.
const BRAND_MARK: f32 = 24.0;

/// How many rows the sidebar's scroller actually paints.
///
/// The tree's row heights are fixed (a 28px workspace header, a 44px session
/// row, a 38px archive header, the 32px tail), so visibility is arithmetic -
/// no measurement cache is needed. A row outside the tree's viewport paints as
/// a same-sized blank, which keeps the scroll height, the wheel and the keys
/// exact while a 400-session install no longer builds 400 rows (menus, icons,
/// hover groups) on every frame.
struct TreeWindow {
    /// The top of the next row, in content coordinates.
    top: f32,
    /// The scroller's viewport in the same coordinates.
    view_top: f32,
    view_h: f32,
}

/// Rows this close to the viewport are built too.
const TREE_WINDOW_OVERDRAW: f32 = 300.0;
/// A tree header (workspace, archived section) is a 28px row.
const TREE_HEADER_H: f32 = 28.0;
/// The archived header adds its 10px separation above the 28px row.
const TREE_ARCHIVED_HEADER_H: f32 = TREE_HEADER_H + 10.0;

impl TreeWindow {
    fn new(scroll: &ScrollHandle, fallback_h: f32) -> Self {
        let height = f32::from(scroll.bounds().size.height);
        Self {
            top: 0.0,
            // Before the first layout the scroller has no height; the window's
            // own height is a safe over-estimate for that one frame.
            view_h: if height > 0.0 { height } else { fallback_h },
            view_top: -f32::from(scroll.offset().y),
        }
    }

    /// Advance past one row, returning whether it must be built.
    fn take(&mut self, height: f32) -> bool {
        let visible = self.top + height > self.view_top - TREE_WINDOW_OVERDRAW
            && self.top < self.view_top + self.view_h + TREE_WINDOW_OVERDRAW;
        self.top += height;
        visible
    }
}

/// A row kept at its known height without building its contents.
fn tree_spacer(height: f32) -> AnyElement {
    div().flex_none().w_full().h(px(height)).into_any_element()
}

impl Shell {
    /// The reference's own DragHandle for the sidebar: an 8px col-resize strip
    /// straddling the seam (margin-left:-4px) at left: cols.sidebar, drawn only
    /// while the column is expanded and working from the pointer's start x.
    pub(crate) fn render_sidebar_handle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("sidebar-handle")
            .debug_selector(|| String::from("sidebar-handle"))
            .occlude()
            .absolute()
            .top(px(0.0))
            .bottom(px(0.0))
            .left(px(self.sidebar_w() - crate::theme::SIDEBAR_HANDLE_W / 2.0))
            .w(px(crate::theme::SIDEBAR_HANDLE_W))
            .cursor(CursorStyle::ResizeLeftRight)
            // The press and the release are the strip's, the way the
            // reference's handle captures its own pointer; the moves go to
            // whatever is under the pointer, so the frame root hears those.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    this.begin_sidebar_drag(event.position.x.as_f32(), cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                    this.commit_sidebar_width(event.position.x.as_f32(), cx);
                }),
            )
    }

    /// The left column.
    pub(crate) fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .debug_selector(|| String::from("sidebar"))
            .w(px(self.sidebar_w()))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .pt(px(6.0))
            .px(px(theme::SIDEBAR_PAD_X))
            .bg(rgb(theme::sidebar_bg()))
            .child(self.titlebar_row(cx))
            .child(self.brand_row())
            .child(self.new_session_button(cx))
            // The tree is the column's only flexible region, so a long session
            // list scrolls INSIDE the sidebar instead of pushing the account row
            // off the bottom of the window.
            .child(self.workspace_tree(cx))
            .child(self.account_row(cx))
    }

    /// The strip the traffic lights sit in, with the sidebar's own fold control.
    ///
    /// The strip is the traffic-light zone itself: the sidebar owns the window's
    /// top-left corner while it is open, so it reserves the measured 52px (see
    /// [`crate::theme::traffic_light_inset`]) and the lights sit in the empty
    /// left of the row, which AppKit keeps draggable as the window's titlebar.
    fn titlebar_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mark = if self.sidebar_folded() {
            IconName::PanelLeftOpen
        } else {
            IconName::PanelLeftClose
        };
        let zone = crate::theme::traffic_light_inset();
        div()
            .h(px(zone.bottom))
            .flex_none()
            .pl(px(zone.right - theme::SIDEBAR_PAD_X))
            .pb(px(2.0))
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .child(
                div()
                    .id("side-fold")
                    .debug_selector(|| String::from("side-fold"))
                    .w(px(28.0))
                    .h(px(28.0))
                    .rounded(px(8.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(theme::hover())))
                    .child(icon(mark, 14.0, theme::muted()))
                    .on_click(cx.listener(|this, _event, window, cx| {
                        this.toggle_sidebar_at(window.viewport_size().width.as_f32(), cx);
                    })),
            )
    }

    /// The brand row, directly under the traffic-light strip.
    fn brand_row(&self) -> impl IntoElement {
        div()
            .debug_selector(|| String::from("side-brand"))
            .h(px(60.0))
            .mt(px(-12.0))
            .mb(px(4.0))
            .py(px(8.0))
            .pl(px(4.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(
                img(crate::assets::mark())
                    .w(px(BRAND_MARK))
                    .h(px(BRAND_MARK))
                    .flex_shrink_0(),
            )
            .child(
                txt("BOS", 18.0, theme::text())
                    .line_height(px(24.0))
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(
                div()
                    .px(px(7.0))
                    .py(px(2.0))
                    .rounded(px(6.0))
                    .bg(rgb(theme::text()))
                    .child(txt("HARNESS", 10.0, theme::page())),
            )
    }

    /// New Session, directly under the brand.
    fn new_session_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        hairline_all(div())
            .id("side-new-session")
            .debug_selector(|| String::from("side-new-session"))
            .mb(px(12.0))
            .mx(px(2.0))
            .h(px(38.0))
            .px(px(12.0))
            .rounded(px(12.0))
            .border_color(rgb(theme::border_soft()))
            .bg(rgb(theme::new_session_fill()))
            .cursor_pointer()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap(px(6.0))
            .hover(|s| s.bg(rgb(theme::hover())))
            .child(icon(IconName::Plus, 15.0, theme::text()))
            .child(
                txt(
                    self.state.lang.tr("side.newSession"),
                    theme::SIDEBAR_SIZE,
                    theme::text(),
                )
                .line_height(px(22.0))
                .font_weight(FontWeight::MEDIUM),
            )
            .on_click(cx.listener(|this, _event, window, cx| {
                this.new_chat(window, cx);
            }))
    }

    /// The workspace tree.
    /// The session-name filter, shown while the header's search control is on.
    fn search_row(&self) -> impl IntoElement {
        div()
            .mx(px(6.0))
            .mb(px(6.0))
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(6.0))
            .bg(rgb(theme::new_session_fill()))
            .border_1()
            .border_color(rgb(theme::border_soft()))
            .child(Input::new(&self.search).appearance(false))
    }

    /// Scroll the workspace tree by `delta` pixels.
    ///
    /// Positive moves towards the top. GPUI stores a scrolled-down offset as a
    /// NEGATIVE y, so the sum is clamped between `-max_offset` and zero rather
    /// than between zero and the max - getting that sign wrong makes the list
    /// jump to its end on the first arrow press.
    pub(crate) fn scroll_tree_by(&mut self, delta: f32, cx: &mut Context<Self>) {
        let max = f32::from(self.side_scroll.max_offset().y);
        if max <= 0.0 {
            return;
        }
        let current = f32::from(self.side_scroll.offset().y);
        let next = (current + delta).clamp(-max, 0.0);
        if (next - current).abs() < f32::EPSILON {
            return;
        }
        self.side_scroll.set_offset(point(px(0.0), px(next)));
        cx.notify();
    }

    fn workspace_tree(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        let mut tree = div()
            .id("side-tree")
            .debug_selector(|| String::from("side-tree"))
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            // The handle the wheel, the keys and the tests all address. Without
            // it the scroller has an offset nobody can read or move from code.
            .track_scroll(&self.side_scroll)
            // A focused list scrolls with the keys; a click anywhere in the tree
            // takes that focus, so the reader does not have to hunt for it.
            .track_focus(&self.side_focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                let page = this.side_scroll.bounds().size.height;
                let Some(delta) = tree_key_delta(event.keystroke.key.as_str(), page) else {
                    return;
                };
                this.scroll_tree_by(delta, cx);
                cx.stop_propagation();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, window, cx| {
                    window.focus(&this.side_focus, cx);
                }),
            )
            .flex()
            .flex_col();
        let mut window = TreeWindow::new(&self.side_scroll, self.viewport_h);

        for workspace in &self.state.settings.global.workspaces {
            let folded = self.state.collapsed.contains(&workspace.id);
            let name = workspace.name.clone();
            let ws_id = workspace.id.clone();
            let row_group = SharedString::from(format!("ws-row-{}", ws_id));
            let click_id = ws_id.clone();
            let add_id = ws_id.clone();
            let more_id = ws_id.clone();
            let menu_items_id = ws_id.clone();
            let row_shell = cx.entity();
            let ws_lang = self.state.lang;
            let renaming = self.renaming_workspace.as_deref() == Some(ws_id.as_str());
            // A row being renamed keeps its field: it is on screen by definition.
            let show = window.take(TREE_HEADER_H) || renaming;
            if !show {
                tree = tree.child(tree_spacer(TREE_HEADER_H));
            } else {
                tree = tree.child(
                    div()
                        .id(SharedString::from(format!("ws-{}", ws_id)))
                        .debug_selector(|| format!("ws-{}", ws_id))
                        .group(row_group.clone())
                        .h(px(28.0))
                        .flex_shrink_0()
                        .px(px(12.0))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(theme::hover())))
                        .child(icon(
                            if folded {
                                IconName::ChevronRight
                            } else {
                                IconName::ChevronDown
                            },
                            13.0,
                            theme::muted(),
                        ))
                        .child(icon(IconName::Folder, 14.0, theme::muted()))
                        .child(if renaming {
                            // The rename happens in place, and the wrapper takes its
                            // own id so a press inside the field does not collapse
                            // the workspace under it.
                            div()
                                .id("workspace-rename")
                                .debug_selector(|| String::from("workspace-rename"))
                                .flex_1()
                                .min_w_0()
                                .on_click(|_event, _window, cx| {
                                    cx.stop_propagation();
                                })
                                .child(
                                    Input::new(&self.workspace_rename)
                                        .id("workspace-rename-field")
                                        .appearance(false)
                                        .h(px(22.0)),
                                )
                                .into_any_element()
                        } else {
                            txt(name, theme::CHIP_SIZE, theme::muted())
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .flex_1()
                                .into_any_element()
                        })
                        // The row's own controls: a chat in this workspace, and the
                        // menu the reference hangs off the ellipsis. They overlay the
                        // right edge and fade in under the pointer, exactly like the
                        // session rows' actions.
                        .child(
                            div()
                                .absolute()
                                .right(px(8.0))
                                .top(px(0.0))
                                .h(px(28.0))
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .id(SharedString::from(format!("ws-new-{}", add_id)))
                                        .debug_selector(|| format!("ws-new-{}", add_id))
                                        .px(px(3.0))
                                        .rounded(px(4.0))
                                        .cursor_pointer()
                                        .opacity(0.0)
                                        .group_hover(row_group.clone(), |s| s.opacity(1.0))
                                        .hover(|s| s.bg(rgb(theme::hover())))
                                        .child(icon(IconName::CirclePlus, 14.0, theme::faint()))
                                        .on_click({
                                            let action_shell = row_shell.clone();
                                            move |_event, _window, cx| {
                                                cx.stop_propagation();
                                                action_shell.update(cx, |this, cx| {
                                                    this.new_session_in_workspace(&add_id, cx);
                                                });
                                            }
                                        }),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!("ws-more-{}", more_id)))
                                        .debug_selector(|| format!("ws-more-{}", more_id))
                                        .opacity(0.0)
                                        .group_hover(row_group, |s| s.opacity(1.0))
                                        .on_click(|_event, _window, cx| {
                                            cx.stop_propagation();
                                        })
                                        .child(
                                            Button::new(SharedString::from(format!(
                                                "workspace-menu-{}",
                                                more_id
                                            )))
                                            .ghost()
                                            .compact()
                                            .icon(IconName::Ellipsis)
                                            .dropdown_menu_with_anchor(
                                                Anchor::TopLeft,
                                                move |menu, _, _| {
                                                    let mut menu = menu;
                                                    for item in
                                                        crate::ui::workspace_menu::workspace_items(
                                                            &menu_items_id,
                                                            ws_lang,
                                                            &row_shell,
                                                        )
                                                    {
                                                        menu = menu.item(item);
                                                    }
                                                    menu
                                                },
                                            ),
                                        ),
                                ),
                        )
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.state.toggle_collapse(&click_id);
                            cx.notify();
                        })),
                );
            }

            if folded {
                continue;
            }
            let mut rows: Vec<_> = self
                .state
                .store
                .in_workspace(&workspace.id)
                .into_iter()
                .filter(|s| crate::state::matches(&self.state.search_query, &s.title))
                .collect();
            crate::store::sort_rows(&mut rows, self.state.sort);
            if rows.is_empty() && !self.state.search_query.trim().is_empty() {
                continue;
            }
            for session in rows {
                let id = session.id.clone();
                // A control the reader is working (an inline rename) is never
                // culled; a list row far outside the tree is.
                let show = window.take(theme::ROW_H)
                    || self.renaming_session.as_deref() == Some(id.as_str());
                if !show {
                    tree = tree.child(tree_spacer(theme::ROW_H));
                    continue;
                }
                let selected = self.state.active_id.as_deref() == Some(id.as_str());
                let streaming = self.state.is_streaming(&id);
                let title = session.title.clone();
                let when = age(session.updated_at);
                let pinned = session.pinned;
                let archived = session.archived_at.is_some();
                let row_lang = self.state.lang;
                let click_id = id.clone();
                let pin_id = id.clone();
                let archive_id = id.clone();
                let stop_id = id.clone();
                let row_shell = cx.entity();
                let row_id = SharedString::from(format!("session-{}", id));
                let row_group = row_id.clone();
                tree = tree.child(
                    div()
                        .debug_selector(|| row_id.to_string())
                        .id(row_id)
                        .group(row_group.clone())
                        .h(px(theme::ROW_H))
                        .flex_shrink_0()
                        .px(px(12.0))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .cursor_pointer()
                        .when(selected, |row| row.bg(rgb(theme::selected_row())))
                        .when(!selected, |row| row.hover(|s| s.bg(rgb(theme::hover()))))
                        .child(
                            div()
                                .w(px(theme::TREE_ROW_LEAD))
                                .h(px(6.0))
                                .rounded_full()
                                .bg(if streaming {
                                    rgb(theme::accent())
                                } else {
                                    rgb(theme::sidebar_bg())
                                }),
                        )
                        .child(if self.renaming_session.as_deref() == Some(id.as_str()) {
                            // The rename happens in place on the row: the
                            // reference's ⌥⌘G puts the field where the title
                            // was, so the reader's eye never leaves the list.
                            // The wrapper takes its own id so a press inside the
                            // field stops here instead of opening the session.
                            div()
                                .id("session-rename")
                                .debug_selector(|| String::from("session-rename"))
                                .flex_1()
                                .min_w_0()
                                .on_click(|_event, _window, cx| {
                                    cx.stop_propagation();
                                })
                                .child(
                                    Input::new(&self.session_rename)
                                        .id("session-rename-field")
                                        .appearance(false)
                                        .h(px(22.0)),
                                )
                                .into_any_element()
                        } else {
                            txt(title, theme::SIDEBAR_SIZE, theme::text())
                                .debug_selector(|| format!("session-title-{}", id))
                                .truncate()
                                .flex_1()
                                .into_any_element()
                        })
                        // The actions overlay the right edge and fade in on hover, which
                        // is what the reference does. In the flow they would take width
                        // from the title and sit mid-row, which is what they used to do.
                        .child(
                            div()
                                .absolute()
                                .right(px(12.0))
                                .top(px(0.0))
                                .h(px(theme::ROW_H))
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .id(SharedString::from(format!("pin-{}", pin_id)))
                                        .debug_selector(|| format!("pin-{}", pin_id))
                                        .px(px(4.0))
                                        .rounded(px(4.0))
                                        .cursor_pointer()
                                        .opacity(if pinned { 1.0 } else { 0.0 })
                                        .group_hover(row_group.clone(), |s| s.opacity(1.0))
                                        .hover(|s| s.bg(rgb(theme::hover())))
                                        .child(icon(
                                            if pinned {
                                                IconName::StarFill
                                            } else {
                                                IconName::Star
                                            },
                                            12.0,
                                            if pinned {
                                                theme::warn()
                                            } else {
                                                theme::faint()
                                            },
                                        ))
                                        .on_click({
                                            let action_shell = row_shell.clone();
                                            move |_event, _window, cx| {
                                                cx.stop_propagation();
                                                action_shell.update(cx, |this, cx| {
                                                    this.state.toggle_pin(&pin_id);
                                                    cx.notify();
                                                });
                                            }
                                        }),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!("archive-{}", archive_id)))
                                        .debug_selector(|| format!("archive-{}", archive_id))
                                        .px(px(4.0))
                                        .rounded(px(4.0))
                                        .cursor_pointer()
                                        .opacity(0.0)
                                        .group_hover(row_group.clone(), |s| s.opacity(1.0))
                                        .hover(|s| s.bg(rgb(theme::hover())))
                                        .child(icon(IconName::Archive, 12.0, theme::faint()))
                                        .on_click({
                                            let action_shell = row_shell.clone();
                                            move |_event, _window, cx| {
                                                cx.stop_propagation();
                                                action_shell.update(cx, |this, cx| {
                                                    this.state.archive_session(&archive_id);
                                                    cx.notify();
                                                });
                                            }
                                        }),
                                )
                                .when(streaming, |row| {
                                    row.child(
                                        div()
                                            .id(SharedString::from(format!("stop-{}", stop_id)))
                                            .debug_selector(|| format!("stop-{}", stop_id))
                                            .px(px(4.0))
                                            .rounded(px(4.0))
                                            .cursor_pointer()
                                            .opacity(0.0)
                                            .group_hover(row_group.clone(), |s| s.opacity(1.0))
                                            .hover(|s| s.bg(rgb(theme::hover())))
                                            .child(icon(IconName::Square, 12.0, theme::error()))
                                            .on_click({
                                                let action_shell = row_shell.clone();
                                                move |_event, _window, cx| {
                                                    cx.stop_propagation();
                                                    action_shell.update(cx, |this, cx| {
                                                        this.state.stop(&stop_id);
                                                        cx.notify();
                                                    });
                                                }
                                            }),
                                    )
                                })
                                .child({
                                    let more_id = id.clone();
                                    let items_id = id.clone();
                                    let menu_shell = row_shell.clone();
                                    let menu_group = row_group.clone();
                                    div()
                                        .id(SharedString::from(format!("more-{}", more_id)))
                                        .debug_selector(|| format!("more-{}", more_id))
                                        .px(px(4.0))
                                        .rounded(px(4.0))
                                        .opacity(0.0)
                                        .group_hover(menu_group, |s| s.opacity(1.0))
                                        .on_click(move |_event, _window, cx| {
                                            cx.stop_propagation();
                                        })
                                        .child(
                                            Button::new(SharedString::from(format!(
                                                "session-menu-{}",
                                                more_id
                                            )))
                                            .ghost()
                                            .compact()
                                            .icon(IconName::Ellipsis)
                                            .dropdown_menu_with_anchor(
                                                Anchor::TopLeft,
                                                move |menu, _, _| {
                                                    let mut menu = menu;
                                                    for item in
                                                        crate::ui::session_menu::session_items(
                                                            &items_id,
                                                            pinned,
                                                            archived,
                                                            streaming,
                                                            row_lang,
                                                            &menu_shell,
                                                        )
                                                    {
                                                        menu = menu.item(item);
                                                    }
                                                    menu
                                                },
                                            ),
                                        )
                                }),
                        )
                        .child(
                            txt(when, theme::CHIP_SIZE, theme::faint())
                                .group_hover(row_group.clone(), |s| s.opacity(0.0)),
                        )
                        .on_click(cx.listener(move |this, _event, window, cx| {
                            this.open_session(&click_id, window, cx);
                        })),
                );
            }
        }

        let archived: Vec<_> = self
            .state
            .store
            .archived()
            .into_iter()
            .map(|session| {
                (
                    session.id.clone(),
                    session.title.clone(),
                    age(session.updated_at),
                )
            })
            .collect();
        if !archived.is_empty() {
            let folded = self.state.collapsed.contains("archived");
            if !window.take(TREE_ARCHIVED_HEADER_H) {
                tree = tree.child(tree_spacer(TREE_ARCHIVED_HEADER_H));
            } else {
                tree = tree.child(
                    div()
                        .id("ws-archived")
                        .debug_selector(|| String::from("ws-archived"))
                        .mt(px(10.0))
                        .h(px(28.0))
                        .flex_shrink_0()
                        .px(px(12.0))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(theme::hover())))
                        .child(icon(
                            if folded {
                                IconName::ChevronRight
                            } else {
                                IconName::ChevronDown
                            },
                            13.0,
                            theme::muted(),
                        ))
                        .child(
                            txt(lang.tr("side.archived"), theme::CHIP_SIZE, theme::muted())
                                .font_weight(FontWeight::SEMIBOLD)
                                .flex_1(),
                        )
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.state.toggle_collapse("archived");
                            cx.notify();
                        })),
                );
            }
            if !folded {
                for (id, title, when) in archived {
                    if !window.take(theme::ROW_H) {
                        tree = tree.child(tree_spacer(theme::ROW_H));
                        continue;
                    }
                    let restore_id = id.clone();
                    // An archived row offers the same menu a live one does, from the
                    // same hover-revealed ellipsis. Its archive cell reads "Unarchive",
                    // because a chat in the archive cannot be archived again.
                    let menu_id = id.clone();
                    let items_id = id.clone();
                    let more_shell = cx.entity();
                    let more_lang = self.state.lang;
                    let row_group = SharedString::from(format!("archived-row-{}", id));
                    tree = tree.child(
                        div()
                            .id(SharedString::from(format!("archived-{}", id)))
                            .debug_selector(|| format!("archived-{}", id))
                            .group(row_group.clone())
                            .h(px(theme::ROW_H))
                            .flex_shrink_0()
                            .px(px(12.0))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            // The same leading column the live rows use, empty: an
                            // archived session is the same session, put away, so it
                            // starts where they start.
                            .child(div().w(px(theme::TREE_ROW_LEAD)).h(px(6.0)).flex_none())
                            .child(
                                txt(title, theme::SIDEBAR_SIZE, theme::muted())
                                    .debug_selector(|| format!("archived-title-{}", id))
                                    .truncate()
                                    .flex_1(),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("restore-{}", restore_id)))
                                    .debug_selector(|| format!("restore-{}", restore_id))
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgb(theme::hover())))
                                    .child(icon(IconName::Undo, 12.0, theme::faint()))
                                    .on_click(cx.listener(move |this, _event, _window, cx| {
                                        this.state.unarchive_session(&restore_id);
                                        cx.notify();
                                    })),
                            )
                            .child(txt(when, theme::CHIP_SIZE, theme::faint()))
                            .child(
                                div()
                                    .id(SharedString::from(format!("more-{}", menu_id)))
                                    .debug_selector(|| format!("more-{}", menu_id))
                                    .opacity(0.0)
                                    .group_hover(row_group, |s| s.opacity(1.0))
                                    .on_click(|_event, _window, cx| {
                                        cx.stop_propagation();
                                    })
                                    .child(
                                        Button::new(SharedString::from(format!(
                                            "session-menu-{}",
                                            menu_id
                                        )))
                                        .ghost()
                                        .compact()
                                        .icon(IconName::Ellipsis)
                                        .dropdown_menu_with_anchor(
                                            Anchor::TopLeft,
                                            move |menu, _, _| {
                                                let mut menu = menu;
                                                for item in crate::ui::session_menu::session_items(
                                                    &items_id,
                                                    false,
                                                    true,
                                                    false,
                                                    more_lang,
                                                    &more_shell,
                                                ) {
                                                    menu = menu.item(item);
                                                }
                                                menu
                                            },
                                        ),
                                    ),
                            ),
                    );
                }
            }
        }

        // The air below the last row: without it the content ends exactly on the
        // account row's divider and the last row reads as sliced by it.
        tree = tree.child(div().h(px(theme::TREE_BOTTOM_INSET)).flex_none());

        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .mt(px(12.0))
            .child(self.workspace_tree_header(cx))
            .when(self.state.search_open, |column| {
                column.child(self.search_row())
            })
            .child(tree)
    }

    /// The Workspaces heading and its three controls.
    ///
    /// Pinned ABOVE the scroller rather than made its first child: the reference
    /// keeps the section title, the search, the sort and the add control in place
    /// while the rows move under them, and a header that scrolls away leaves the
    /// list looking like it has no name.
    fn workspace_tree_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let shell = cx.entity();
        let lang = self.state.lang;
        div().child(
            div()
                .px(px(12.0))
                .pb(px(4.0))
                .flex()
                .flex_row()
                .items_center()
                .child(txt(lang.tr("side.workspaces"), theme::CHIP_SIZE, theme::muted()).flex_1())
                .child(
                    div()
                        .id("side-search")
                        .debug_selector(|| String::from("side-search"))
                        .mr(px(2.0))
                        .px(px(4.0))
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .when(self.state.search_open, |s| s.bg(rgb(theme::selected_row())))
                        .hover(|s| s.bg(rgb(theme::hover())))
                        .child(icon(
                            IconName::Search,
                            14.0,
                            if self.state.search_open {
                                theme::text()
                            } else {
                                theme::faint()
                            },
                        ))
                        .on_click(cx.listener(|this, _event, window, cx| {
                            this.state.search_open = !this.state.search_open;
                            if !this.state.search_open {
                                this.state.search_query.clear();
                                this.search.update(cx, |state, cx| {
                                    state.set_value(String::new(), window, cx);
                                });
                            }
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("side-sort")
                        .ghost()
                        .compact()
                        .mr(px(2.0))
                        .icon(IconName::ArrowUpDown)
                        .dropdown_menu_with_anchor(Anchor::TopLeft, {
                            let sort_shell = shell.clone();
                            let current = self.state.sort;
                            let sort_lang = self.state.lang;
                            move |menu, _, _| {
                                let mut menu = menu;
                                for option in [
                                    crate::store::SessionSort::Updated,
                                    crate::store::SessionSort::Created,
                                    crate::store::SessionSort::Name,
                                ] {
                                    let pick = sort_shell.clone();
                                    let checked = option.key() == current.key();
                                    menu =
                                        menu.item(
                                            PopupMenuItem::new(crate::i18n::tr(
                                                sort_lang,
                                                option.key(),
                                            ))
                                            .checked(checked)
                                            .on_click(move |_, _window, cx| {
                                                pick.update(cx, |this, cx| {
                                                    this.state.sort = option;
                                                    this.state
                                                        .settings
                                                        .global
                                                        .general
                                                        .session_sort =
                                                        option.persisted().to_string();
                                                    let _ = this.state.settings.save();
                                                    cx.notify();
                                                });
                                            }),
                                        );
                                }
                                menu
                            }
                        }),
                )
                .child(
                    div()
                        .id("side-add-workspace")
                        .debug_selector(|| String::from("side-add-workspace"))
                        .px(px(4.0))
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(theme::hover())))
                        .child(icon(IconName::Plus, 14.0, theme::faint()))
                        .on_click(cx.listener(|this, _event, window, cx| {
                            this.add_workspace(window, cx);
                        })),
                ),
        )
    }

    /// The bottom account row.
    fn account_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        let shell = cx.entity();
        div()
            .debug_selector(|| String::from("account-row"))
            // Pinned: the row is the sidebar's floor, so it keeps its height no
            // matter how long the tree above it grows.
            .flex_none()
            .h(px(44.0))
            .px(px(6.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .border_t_1()
            .border_color(rgb(theme::border()))
            .child(
                div()
                    .w(px(22.0))
                    .h(px(22.0))
                    .rounded_full()
                    .bg(rgb(theme::accent()))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(txt("B", 12.0, theme::page()).font_weight(FontWeight::BOLD)),
            )
            .child(
                Button::new("account-menu")
                    .ghost()
                    .compact()
                    .label(lang.tr("side.local"))
                    .flex_1()
                    .dropdown_menu_with_anchor(Anchor::TopLeft, {
                        let lang_shell = shell.clone();
                        let theme_shell = shell.clone();
                        let settings_shell = shell.clone();
                        let lang_label = crate::i18n::tr(lang, "settings.language");
                        let theme_label = crate::i18n::tr(lang, "settings.theme");
                        let settings_label = crate::i18n::tr(lang, "menu.settings");
                        move |menu, _, _| {
                            let toggle_shell = lang_shell.clone();
                            let look_shell = theme_shell.clone();
                            let open_shell = settings_shell.clone();
                            menu.item(PopupMenuItem::new(lang_label.clone()).on_click(
                                move |_, _window, cx| {
                                    toggle_shell.update(cx, |this, cx| {
                                        this.state.lang.toggle();
                                        this.state.settings.global.general.lang =
                                            this.state.lang.locale().to_string();
                                        let _ = this.state.settings.save();
                                        cx.notify();
                                    });
                                },
                            ))
                            .item(PopupMenuItem::new(theme_label.clone()).on_click(
                                move |_, window, cx| {
                                    look_shell.update(cx, |this, cx| {
                                        crate::theme::toggle(window, cx);
                                        this.state.settings.global.general.theme_mode =
                                            crate::theme::mode(cx).to_string();
                                        let _ = this.state.settings.save();
                                        cx.notify();
                                    });
                                },
                            ))
                            .item(PopupMenuItem::separator())
                            .item(
                                PopupMenuItem::new(settings_label.clone()).on_click(
                                    move |_, window, cx| {
                                        open_shell.update(cx, |this, cx| {
                                            this.open_settings(window, cx);
                                        });
                                    },
                                ),
                            )
                        }
                    }),
            )
            .child(self.sidebar_icon_button(
                "side-settings",
                icon(IconName::Settings, 15.0, theme::muted()),
                cx,
            ))
            // The bin: the last thing in the row, which is where a reader looks for
            // "the things I deleted" rather than among the app's preferences.
            .child(
                div()
                    .debug_selector(|| String::from("side-trash"))
                    .id("side-trash")
                    .w(px(28.0))
                    .h(px(28.0))
                    .rounded(px(8.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(theme::hover())))
                    .child(icon(IconName::Trash, 15.0, theme::muted()))
                    .on_click(cx.listener(|this, _event, _window, cx| this.open_trash(cx))),
            )
    }

    /// A small square icon button in the account row.
    fn sidebar_icon_button(
        &self,
        id: &'static str,
        mark: impl IntoElement,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .debug_selector(|| id.to_string())
            .id(id)
            .w(px(28.0))
            .h(px(28.0))
            .rounded(px(8.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(theme::hover())))
            .child(mark)
            .on_click(cx.listener(move |this, _event, window, cx| this.open_settings(window, cx)))
    }
}
