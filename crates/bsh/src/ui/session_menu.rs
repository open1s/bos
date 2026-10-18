//! The session actions, shared by the sidebar row and the header More actions.
//!
//! The set is the reference's own: pin, rename, fork, archive, copy the id.
//! Each row carries an icon and, where a real accelerator exists, its keys - a
//! hint is only drawn when pressing those keys does the same thing, which is why
//! the actions below are bound in `ui::bind_global_actions`.

use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::*;

use super::shell::Shell;
use super::{icon, txt, IconName};
use crate::i18n::{tr, Lang};
use crate::theme;

actions!(
    bsh_session,
    [RenameSession, ForkSession, ArchiveSession, CopySessionId]
);

/// The accelerator labels the menu shows, matching `bind_global_actions`.
pub(crate) const RENAME_KEYS: &str = "\u{2325}\u{2318}G";
/// See [`RENAME_KEYS`].
pub(crate) const FORK_KEYS: &str = "\u{2325}\u{2318}F";
/// See [`RENAME_KEYS`].
pub(crate) const ARCHIVE_KEYS: &str = "\u{21e7}\u{2318}A";

/// One menu row: leading glyph, label, trailing accelerator.
///
/// A plain `PopupMenuItem::new` cannot carry the trailing keys - the kit's item
/// has a label and an icon slot and nothing else - so a row with an accelerator
/// is an element. Rows without one still use the element form so every line in
/// the menu shares one baseline.
fn action_row(
    selector: &'static str,
    icon_name: IconName,
    label: String,
    keys: Option<&'static str>,
) -> PopupMenuItem {
    action_row_tinted(selector, icon_name, label, keys, false)
}

/// The same row for a destructive cell: the label and its glyph take the error
/// colour, which the kit's own item cannot be told to do.
fn action_row_danger(selector: &'static str, icon_name: IconName, label: String) -> PopupMenuItem {
    action_row_tinted(selector, icon_name, label, None, true)
}

fn action_row_tinted(
    selector: &'static str,
    icon_name: IconName,
    label: String,
    keys: Option<&'static str>,
    danger: bool,
) -> PopupMenuItem {
    PopupMenuItem::element(move |_window, _cx| {
        let colour = if danger {
            theme::error()
        } else {
            theme::text()
        };
        let glyph = if danger { colour } else { theme::muted() };
        let mut row = div()
            .debug_selector(move || selector.to_string())
            .flex()
            .flex_row()
            .items_center()
            .gap(px(9.0))
            .w_full()
            .child(icon(icon_name, 14.0, glyph).flex_none())
            .child(txt(label.clone(), 13.0, colour).flex_1().min_w_0());
        if let Some(keys) = keys {
            row = row.child(
                txt(keys, 12.0, theme::faint())
                    .flex_none()
                    .font_family("monospace"),
            );
        }
        row
    })
}

/// The actions a session offers, as menu items for whichever door opened it.
pub(crate) fn session_items(
    id: &str,
    pinned: bool,
    archived: bool,
    streaming: bool,
    lang: Lang,
    shell: &Entity<Shell>,
) -> Vec<PopupMenuItem> {
    let pin_id = id.to_string();
    let rename_id = id.to_string();
    let fork_id = id.to_string();
    let archive_id = id.to_string();
    let copy_id = id.to_string();
    let delete_id = id.to_string();
    let stop_id = id.to_string();
    let pin_shell = shell.clone();
    let rename_shell = shell.clone();
    let fork_shell = shell.clone();
    let archive_shell = shell.clone();
    let copy_shell = shell.clone();
    let delete_shell = shell.clone();
    let stop_shell = shell.clone();
    let mut items = vec![
        action_row(
            "session-menu-pin",
            if pinned {
                IconName::StarFill
            } else {
                IconName::Star
            },
            tr(lang, if pinned { "menu.unpin" } else { "menu.pin" }),
            None,
        )
        .on_click(move |_, _window, cx| {
            pin_shell.update(cx, |this, cx| {
                this.state.toggle_pin(&pin_id);
                cx.notify();
            });
        }),
        action_row(
            "session-menu-rename",
            IconName::Pencil,
            tr(lang, "menu.rename"),
            Some(RENAME_KEYS),
        )
        .on_click(move |_, window, cx| {
            rename_shell.update(cx, |this, cx| {
                this.begin_session_rename(&rename_id, window, cx);
            });
        }),
        action_row(
            "session-menu-fork",
            IconName::SquarePlus,
            tr(lang, "menu.fork"),
            Some(FORK_KEYS),
        )
        .on_click(move |_, _window, cx| {
            fork_shell.update(cx, |this, cx| {
                this.fork_session_at(&fork_id, cx);
            });
        }),
        // The archive cell is the one that changes with the row: an archived chat
        // cannot be archived again, so the same slot offers the way back. It
        // carries no accelerator because none is bound to unarchiving - a hint is
        // only drawn when pressing those keys does the same thing.
        if archived {
            action_row(
                "session-menu-unarchive",
                IconName::Undo,
                tr(lang, "menu.unarchive"),
                None,
            )
            .on_click(move |_, _window, cx| {
                archive_shell.update(cx, |this, cx| {
                    this.state.unarchive_session(&archive_id);
                    cx.notify();
                });
            })
        } else {
            action_row(
                "session-menu-archive",
                IconName::Archive,
                tr(lang, "menu.archive"),
                Some(ARCHIVE_KEYS),
            )
            .on_click(move |_, _window, cx| {
                archive_shell.update(cx, |this, cx| {
                    this.state.archive_session(&archive_id);
                    cx.notify();
                });
            })
        },
        action_row(
            "session-menu-copy",
            IconName::Copy,
            tr(lang, "menu.copyId"),
            None,
        )
        .on_click(move |_, _window, cx| {
            copy_shell.update(cx, |this, cx| {
                this.copy_session_id(&copy_id, cx);
            });
        }),
        // A rule, then the one action that cannot be undone: archive keeps the
        // transcript, this deletes it, and the reference separates the two kinds
        // so a reader cannot pick the wrong one by muscle memory.
        PopupMenuItem::separator(),
        action_row_danger(
            "session-menu-delete",
            IconName::Trash,
            tr(lang, "menu.deleteSession"),
        )
        .on_click(move |_, _window, cx| {
            delete_shell.update(cx, |this, cx| {
                this.delete_session(&delete_id, cx);
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
