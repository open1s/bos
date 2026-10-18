//! The workspace row's own menu.
//!
//! Measured from the reference's sidebar: a workspace row reveals two controls
//! under the pointer - a circled plus, which opens a chat in that workspace, and
//! an ellipsis, which opens this menu. Its cells are "Rename" with a pencil and
//! "Delete workspace" with a trash glyph in the error colour.

use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::*;

use super::shell::Shell;
use super::{icon, txt, IconName};
use crate::i18n::{tr, Lang};
use crate::theme;

/// One menu row, drawn as an element so the destructive cell can carry the
/// error colour - the kit's own item paints its label from the theme and takes
/// no tint.
fn row(selector: &'static str, icon_name: IconName, label: String, danger: bool) -> PopupMenuItem {
    PopupMenuItem::element(move |_window, _cx| {
        let colour = if danger {
            theme::error()
        } else {
            theme::text()
        };
        div()
            .debug_selector(move || selector.to_string())
            .flex()
            .flex_row()
            .items_center()
            .gap(px(9.0))
            .w_full()
            .child(icon(icon_name, 14.0, colour).flex_none())
            .child(txt(label.clone(), 13.0, colour).flex_1().min_w_0())
    })
}

/// The cells a workspace row offers.
pub(crate) fn workspace_items(id: &str, lang: Lang, shell: &Entity<Shell>) -> Vec<PopupMenuItem> {
    let rename_shell = shell.clone();
    let rename_id = id.to_string();
    let delete_shell = shell.clone();
    let delete_id = id.to_string();
    vec![
        row(
            "ws-menu-rename",
            IconName::Pencil,
            tr(lang, "menu.rename"),
            false,
        )
        .on_click(move |_, window, cx| {
            let shell = rename_shell.clone();
            let id = rename_id.clone();
            shell.update(cx, |this, cx| this.begin_workspace_rename(&id, window, cx));
        }),
        row(
            "ws-menu-delete",
            IconName::Trash,
            tr(lang, "menu.deleteWorkspace"),
            true,
        )
        .on_click(move |_, _window, cx| {
            let shell = delete_shell.clone();
            let id = delete_id.clone();
            shell.update(cx, |this, cx| this.delete_workspace(&id, cx));
        }),
    ]
}
