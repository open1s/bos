//! The session actions, shared by the sidebar row and the header More actions.

use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::*;

use super::shell::Shell;
use crate::i18n::{tr, Lang};

/// The actions a session offers, as menu items for whichever door opened it.
pub(crate) fn session_items(
    id: &str,
    pinned: bool,
    streaming: bool,
    lang: Lang,
    shell: &Entity<Shell>,
) -> Vec<PopupMenuItem> {
    let pin_id = id.to_string();
    let archive_id = id.to_string();
    let stop_id = id.to_string();
    let pin_shell = shell.clone();
    let archive_shell = shell.clone();
    let stop_shell = shell.clone();
    let mut items = vec![
        PopupMenuItem::new(tr(lang, if pinned { "menu.unpin" } else { "menu.pin" }))
            .checked(pinned)
            .on_click(move |_, _window, cx| {
                pin_shell.update(cx, |this, cx| {
                    this.state.toggle_pin(&pin_id);
                    cx.notify();
                });
            }),
        PopupMenuItem::new(tr(lang, "menu.archive")).on_click(move |_, _window, cx| {
            archive_shell.update(cx, |this, cx| {
                this.state.archive_session(&archive_id);
                cx.notify();
            });
        }),
    ];
    if streaming {
        items.push(
            PopupMenuItem::new(tr(lang, "menu.stop")).on_click(move |_, _window, cx| {
                stop_shell.update(cx, |this, cx| {
                    this.state.stop(&stop_id);
                    cx.notify();
                });
            }),
        );
    }
    items
}
