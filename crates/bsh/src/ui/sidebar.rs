//! The sidebar: brand, New Session, global panels, the workspace tree, account.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::*;

use super::shell::Shell;
use super::{hairline_all, icon, txt, DivExt, IconName};
use crate::state::age;
use crate::theme;

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
            .px(px(12.0))
            .bg(rgb(theme::sidebar_bg()))
            .child(self.titlebar_row(cx))
            .child(self.brand_row())
            .child(self.new_session_button(cx))
            .child(self.panels_nav(cx))
            .child(self.workspace_tree(cx))
            .child(div().flex_1())
            .child(self.account_row(cx))
    }

    /// The strip the traffic lights sit in, with the sidebar's own fold control.
    fn titlebar_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mark = if self.sidebar_folded() {
            IconName::PanelLeftOpen
        } else {
            IconName::PanelLeftClose
        };
        div()
            .h(px(52.0))
            .flex_none()
            .pl(px(72.0))
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
                img(crate::assets::logo())
                    .w(px(20.0))
                    .h(px(20.0))
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

    /// Plugins / Automation / MCP.
    fn panels_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        let row = |id: &'static str,
                   key: &'static str,
                   section: Option<&'static str>,
                   mark: IconName,
                   cx: &mut Context<Self>| {
            // A row names a panel, so it opens that section. A row with none has
            // no panel in this deployment - the reference schedules automation
            // tasks and bsh composes no task scheduler - so it says so rather than
            // landing on a pane that only looks related.
            let section = section.and_then(super::settings::section_index);
            div()
                .debug_selector(|| id.to_string())
                .id(id)
                .mx(px(2.0))
                .min_h(px(36.0))
                .px(px(8.0))
                .py(px(7.0))
                .rounded(px(12.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(theme::hover())))
                .child(icon(mark, 14.0, theme::muted()))
                .child(txt(lang.tr(key), theme::SIDEBAR_SIZE, theme::text()).line_height(px(22.0)))
                .on_click(cx.listener(move |this, _event, window, cx| match section {
                    Some(index) => this.open_settings_at(index, window, cx),
                    None => {
                        let text = this.state.lang.tr("nav.automationUnavailable").to_string();
                        this.state.notice(text);
                        cx.notify();
                    }
                }))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .mb(px(8.0))
            .child(row(
                "nav-plugins",
                "nav.plugins",
                Some("plugins"),
                IconName::Puzzle,
                cx,
            ))
            .child(row(
                "nav-automation",
                "nav.automation",
                Some("automation"),
                IconName::Clock,
                cx,
            ))
            .child(row("nav-mcp", "nav.mcp", Some("mcp"), IconName::Plug, cx))
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

    fn workspace_tree(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let shell = cx.entity();
        let lang = self.state.lang;
        let mut tree = div().mt(px(12.0)).flex().flex_col().child(
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
                                                    this.state.settings.session_sort =
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
        );

        if self.state.search_open {
            tree = tree.child(self.search_row());
        }

        for workspace in &self.state.settings.workspaces {
            let folded = self.state.collapsed.contains(&workspace.id);
            let name = workspace.name.clone();
            let ws_id = workspace.id.clone();
            tree = tree.child(
                div()
                    .id(SharedString::from(format!("ws-{}", ws_id)))
                    .debug_selector(|| format!("ws-{}", ws_id))
                    .h(px(28.0))
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
                    .child(
                        txt(name, theme::CHIP_SIZE, theme::muted())
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .flex_1(),
                    )
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.state.toggle_collapse(&ws_id);
                        cx.notify();
                    })),
            );

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
                let selected = self.state.active_id.as_deref() == Some(id.as_str());
                let streaming = self.state.is_streaming(&id);
                let title = session.title.clone();
                let when = age(session.updated_at);
                let pinned = session.pinned;
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
                        .px(px(12.0))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .cursor_pointer()
                        .when(selected, |row| row.bg(rgb(theme::selected_row())))
                        .when(!selected, |row| row.hover(|s| s.bg(rgb(theme::hover()))))
                        .child(div().w(px(6.0)).h(px(6.0)).rounded_full().bg(if streaming {
                            rgb(theme::accent())
                        } else {
                            rgb(theme::sidebar_bg())
                        }))
                        .child(
                            txt(title, theme::SIDEBAR_SIZE, theme::text())
                                .truncate()
                                .flex_1(),
                        )
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
            tree = tree.child(
                div()
                    .id("ws-archived")
                    .debug_selector(|| String::from("ws-archived"))
                    .mt(px(10.0))
                    .h(px(28.0))
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
            if !folded {
                for (id, title, when) in archived {
                    let restore_id = id.clone();
                    tree = tree.child(
                        div()
                            .id(SharedString::from(format!("archived-{}", id)))
                            .debug_selector(|| format!("archived-{}", id))
                            .h(px(theme::ROW_H))
                            .px(px(12.0))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                txt(title, theme::SIDEBAR_SIZE, theme::muted())
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
                            .child(txt(when, theme::CHIP_SIZE, theme::faint())),
                    );
                }
            }
        }
        tree
    }

    /// The bottom account row.
    fn account_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        let shell = cx.entity();
        div()
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
                                        this.state.settings.lang =
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
                                        this.state.settings.theme_mode =
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
