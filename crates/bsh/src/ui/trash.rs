//! The recycle bin: what was deleted, and the way back.
//!
//! One overlay, reached from the account row's bin control. It lists the parked
//! chats and workspaces newest-first with the workspace they came from, and every
//! row carries Restore and Delete permanently - the two decisions a bin exists to
//! offer.

use gpui_kit::*;

use super::shell::Shell;
use super::{hairline_all, icon, txt, IconName};
use crate::i18n::tr;
use crate::theme;
use crate::trash::{Entry, Kind};

impl Shell {
    /// Open the bin, reading it fresh: the file system is the truth and the panel
    /// is a view of it.
    pub(crate) fn open_trash(&mut self, cx: &mut Context<Self>) {
        self.trash_open = true;
        self.trash_entries = self.state.trash();
        cx.notify();
    }

    /// Close it without touching anything parked.
    pub(crate) fn close_trash(&mut self, cx: &mut Context<Self>) {
        self.trash_open = false;
        cx.notify();
    }

    /// Put a parked item back, then re-read the bin.
    pub(crate) fn restore_trashed(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        self.state.restore_trashed(path);
        self.trash_entries = self.state.trash();
        cx.notify();
    }

    /// Delete a parked item for good, then re-read the bin.
    pub(crate) fn purge_trashed(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        self.state.purge_trashed(path);
        self.trash_entries = self.state.trash();
        cx.notify();
    }

    /// The bin overlay.
    pub(crate) fn render_trash(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let entries = self.trash_entries.clone();
        let sessions: Vec<Entry> = entries
            .iter()
            .filter(|entry| entry.kind() == Some(Kind::Session))
            .cloned()
            .collect();
        let workspaces: Vec<Entry> = entries
            .iter()
            .filter(|entry| entry.kind() == Some(Kind::Workspace))
            .cloned()
            .collect();

        let mut body = div()
            .id("trash-list")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(2.0));
        if entries.is_empty() {
            body = body.child(
                txt(tr(lang, "trash.empty"), 13.0, theme::faint())
                    .py(px(18.0))
                    .px(px(2.0)),
            );
        }
        let groups = [
            (tr(lang, "trash.sessions"), sessions),
            (tr(lang, "trash.workspaces"), workspaces),
        ];
        for (index, (title, rows)) in groups.into_iter().enumerate() {
            if rows.is_empty() {
                continue;
            }
            body = body.child(
                txt(title, 12.0, theme::muted())
                    .pt(px(8.0))
                    .pb(px(4.0))
                    .px(px(2.0)),
            );
            for entry in rows {
                body = body.child(self.trash_row(index, &entry, cx));
            }
        }

        let card = div()
            .debug_selector(|| String::from("trash-panel"))
            .w(px(460.0))
            .h(px(420.0))
            .rounded(px(16.0))
            .bg(rgb(theme::page()))
            .shadow_lg()
            .p(px(16.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(icon(IconName::Trash, 16.0, theme::error()))
                    .child(
                        txt(tr(lang, "trash.title"), 15.0, theme::text())
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        div()
                            .id("trash-close")
                            .debug_selector(|| String::from("trash-close"))
                            .w(px(24.0))
                            .h(px(24.0))
                            .rounded(px(6.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(theme::hover())))
                            .child(icon(IconName::Close, 14.0, theme::muted()))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.close_trash(cx);
                            })),
                    ),
            )
            .child(body);
        // The backdrop is the close target: a press outside the card dismisses the
        // panel, which is what every overlay in this shell does.
        div()
            .id("trash-backdrop")
            .debug_selector(|| String::from("trash-backdrop"))
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .size_full()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000088))
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.close_trash(cx);
            }))
            .child(
                div()
                    .id("trash-card-hit")
                    .on_click(|_event, _window, cx| cx.stop_propagation())
                    .child(card),
            )
            .into_any_element()
    }

    /// One parked item: what it was, where it came from, and its two actions.
    fn trash_row(&self, group: usize, entry: &Entry, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let path = entry.path.clone();
        let purge_path = entry.path.clone();
        let selector: &'static str = match (group, entry.kind()) {
            (0, _) => "trash-session-row",
            (_, Some(Kind::Workspace)) => "trash-workspace-row",
            _ => "trash-row",
        };
        let meta = format!(
            "{}  ·  {}",
            entry.envelope.workspace_name,
            crate::state::age(entry.envelope.deleted_at)
        );
        let row = div()
            .debug_selector(move || selector.to_string())
            .h(px(36.0))
            .px(px(8.0))
            .rounded(px(8.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .hover(|s| s.bg(rgb(theme::hover())))
            .child(
                txt(entry.envelope.name.clone(), 13.0, theme::text())
                    .flex_1()
                    .min_w_0()
                    .truncate(),
            )
            .child(txt(meta, 11.0, theme::faint()).flex_none())
            .child(
                div()
                    .id(SharedString::from(format!(
                        "trash-restore-{}",
                        entry.path.display()
                    )))
                    .debug_selector(|| String::from("trash-restore"))
                    .px(px(8.0))
                    .py(px(3.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .border_1()
                    .border_color(rgb(theme::border()))
                    .child(txt(tr(lang, "trash.restore"), 12.0, theme::text()))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.restore_trashed(&path, cx);
                    })),
            )
            .child(
                div()
                    .id(SharedString::from(format!(
                        "trash-purge-{}",
                        entry.path.display()
                    )))
                    .debug_selector(|| String::from("trash-purge"))
                    .w(px(24.0))
                    .h(px(24.0))
                    .rounded(px(6.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(theme::hover())))
                    .child(icon(IconName::Trash, 13.0, theme::error()))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.purge_trashed(&purge_path, cx);
                    })),
            );
        hairline_all(row)
            .border_color(rgb(theme::border_soft()))
            .into_any_element()
    }
}
