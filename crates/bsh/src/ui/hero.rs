//! The reference's hero: the empty conversation's headline and workspace chip.
//!
//! Measured from `HeroShell.module.css` and the `HeroShell`/`WorkspaceChip`
//! components in `dsh-client-ui-conversation`. The reference draws the hero in
//! the composer stack, not in the transcript, so `Shell::render_main_column`
//! centres the group while `AppState::hero_phase` holds, and the transcript
//! renders nothing.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::*;

use super::shell::Shell;
use super::{hairline_all, icon, txt, DivExt, IconName};
use crate::theme;

impl Shell {
    /// The hero headline: the brand mark, the headline and the preview badge.
    ///
    /// Measured: a centred root padded 24 a side holds a stack at most one
    /// composer card wide, whose headline is a wrapping centred row at a
    /// 12/10 gap over a title group at 4/7. The mark is 34 wide in
    /// label-primary; the badge is a mono 12/18 pill at radius 999 with a
    /// 1px top pad and a `.5px` border, over state-business-tertiary in
    /// label-primary-bluish.
    pub(crate) fn hero(&self) -> impl IntoElement {
        let lang = self.state.lang;
        let headline = txt(
            lang.tr("hero.headline"),
            theme::HERO_HEADLINE_SIZE,
            theme::text(),
        )
        .font_weight(FontWeight::MEDIUM)
        .line_height(px(theme::HERO_HEADLINE_LINE));
        div()
            .debug_selector(|| String::from("hero"))
            .w_full()
            .flex()
            .justify_center()
            .px(px(theme::HERO_PAD_X))
            .child(
                div()
                    .w_full()
                    .max_w(px(self.composer_w()))
                    .flex()
                    .flex_col()
                    // The reference's stack always holds its empty body as a
                    // second row, so the measured 12px gap runs out below the
                    // headline before the workspace row's own 4px top margin.
                    .pb(px(theme::HERO_STACK_GAP))
                    .child(
                        div()
                            .debug_selector(|| String::from("hero-headline"))
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .justify_center()
                            .gap(px(theme::HERO_HEADLINE_GAP))
                            // The measured box stays [`theme::HERO_MARK`] so the
                            // headline keeps its line box; the emblem is drawn
                            // larger inside it and centred, which costs no layout
                            // height because a child does not push a fixed box.
                            .child(
                                div()
                                    .w(px(theme::HERO_MARK))
                                    .h(px(theme::HERO_MARK))
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        img(crate::assets::mark())
                                            .w(px(theme::HERO_MARK_DRAWN))
                                            .h(px(theme::HERO_MARK_DRAWN)),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .justify_center()
                                    .gap(px(theme::HERO_TITLE_GAP))
                                    .child(headline)
                                    .child(self.hero_preview_badge()),
                            ),
                    ),
            )
    }

    /// The preview pill the headline carries.
    fn hero_preview_badge(&self) -> impl IntoElement {
        let badge = div()
            .mt(px(theme::HERO_BADGE_TOP))
            .px(px(theme::HERO_BADGE_PAD_X))
            .pt(px(theme::HERO_BADGE_PAD_Y))
            .rounded(px(999.0))
            .bg(rgb(theme::hero_badge_bg()))
            .child(
                txt(
                    self.state.lang.tr("hero.preview"),
                    theme::HERO_BADGE_SIZE,
                    theme::hero_badge_fg(),
                )
                .font_weight(FontWeight::MEDIUM)
                .line_height(px(theme::HERO_BADGE_LINE)),
            );
        hairline_all(badge).border_color(rgb(theme::hero_badge_border()))
    }

    /// The hero's workspace chip: the folder, the owning workspace and a
    /// chevron that opens the picker.
    ///
    /// Measured: the row is a 2px-gap flex line 20px in on the left and 16 on
    /// the right, 4px under the headline; the chip is at minimum 28 tall at
    /// radius-sm, 13/20 weight 500 in label-primary, 8px a side over a 4px
    /// gap, with a 16px folder and a 12px chevron in label-caption. The chip
    /// is absent while the active session owns no workspace.
    pub(crate) fn hero_workspace_row(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let lang = self.state.lang;
        let current = self
            .state
            .active_session()
            .map(|session| session.workspace.clone())
            .filter(|id| !id.is_empty())?;
        let named = self
            .state
            .settings
            .global
            .workspaces
            .iter()
            .find(|workspace| workspace.id == current)
            .map(|workspace| workspace.name.clone());
        // Measured: the reference keeps the closed folder while the chip still
        // shows its "Choose workspace" prompt and opens it once a workspace is
        // named.
        let glyph = if named.is_some() {
            IconName::FolderOpen
        } else {
            IconName::FolderClosed
        };
        let label = named.unwrap_or_else(|| lang.tr("hero.chooseWorkspace"));
        let workspaces: Vec<(String, String)> = self
            .state
            .settings
            .global
            .workspaces
            .iter()
            .map(|workspace| (workspace.id.clone(), workspace.name.clone()))
            .collect();
        let shell = cx.entity();
        let chip = Button::new("hero-workspace")
            .ghost()
            .h(px(theme::HERO_WS_MIN_H))
            .px(px(theme::HERO_WS_PAD_X))
            .rounded(px(theme::RADIUS_SM))
            .gap(px(theme::HERO_WS_GAP))
            .max_w(px(theme::HERO_WS_MAX_W))
            .child(icon(glyph, theme::HERO_WS_ICON, theme::text()))
            .child(
                txt(label, theme::HERO_WS_SIZE, theme::text())
                    .font_weight(FontWeight::MEDIUM)
                    .line_height(px(theme::HERO_WS_LINE))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap(),
            )
            .child(icon(
                IconName::ChevronDown,
                theme::HERO_WS_CHEVRON,
                theme::caption(),
            ))
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                let mut menu = menu;
                for (id, name) in workspaces.iter() {
                    let pick = shell.clone();
                    let id = id.clone();
                    let checked = *id == current;
                    menu = menu.item(PopupMenuItem::new(name.clone()).checked(checked).on_click(
                        move |_, _window, cx| {
                            let id = id.clone();
                            pick.update(cx, |this, cx| {
                                this.state.set_workspace(&id);
                                // The file pane's paths are relative, so the
                                // switch rebases them onto another workspace.
                                this.refile_workspace(cx);
                                cx.notify();
                            });
                        },
                    ));
                }
                // The reference closes the list with a separator and the door to
                // a new root; without it the only way to add one was the
                // sidebar's small plus.
                let add = shell.clone();
                let add_label = lang.tr("side.addWorkspace");
                menu = menu.item(PopupMenuItem::separator()).item(
                    PopupMenuItem::element(move |_window, _cx| {
                        div()
                            .debug_selector(|| String::from("hero-add-workspace"))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(9.0))
                            .w_full()
                            .child(icon(IconName::Plus, 14.0, theme::muted()).flex_none())
                            .child(txt(add_label.clone(), 13.0, theme::text()).flex_1())
                    })
                    .on_click(move |_, window, cx| {
                        let add = add.clone();
                        add.update(cx, |this, cx| this.add_workspace(window, cx));
                    }),
                );
                menu
            });
        Some(
            div()
                .debug_selector(|| String::from("hero-workspace-row"))
                .w_full()
                .flex()
                .items_center()
                .gap(px(theme::HERO_WS_ROW_GAP))
                .mt(px(theme::HERO_WS_ROW_TOP))
                .pl(px(theme::HERO_WS_ROW_PAD_L))
                .pr(px(theme::HERO_WS_ROW_PAD_R))
                .child(
                    div()
                        .debug_selector(|| String::from("hero-workspace"))
                        .child(chip),
                )
                .child(self.agent_preset_chip(self.state.lang, cx)),
        )
    }

    /// The hero phase's composer stack: the headline, the workspace chip and
    /// the composer in one centred column.
    ///
    /// Measured: the reference centres the scroll body and lays the stack out
    /// at `min(card + 2 * clearance, 100%)` over an 8px gap with 32px of
    /// bottom padding; the composer card itself stays one clearance narrower
    /// than that stack.
    pub(crate) fn hero_group(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .debug_selector(|| String::from("hero-group"))
            .w(px(self.hero_w()))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(theme::HERO_GROUP_GAP))
            .pb(px(theme::HERO_GROUP_PB))
            .child(self.hero())
            .when_some(self.hero_workspace_row(cx), |group, row| group.child(row))
            .child(
                div()
                    .w(px(self.composer_w()))
                    .flex()
                    .flex_col()
                    .child(self.render_composer(cx)),
            )
    }
}
