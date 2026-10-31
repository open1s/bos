//! The GPUI renderer for declarative extension panels.
//!
//! This view owns no extension state beyond the selected revision. It asks the
//! owning Runtime for a UiDocument, validates it again at the host boundary,
//! renders the bounded tree with GPUI widgets, and routes every action back with
//! the revision it was rendered from. A stale action is refused, so a button
//! from an old frame cannot mutate a newer one.

use serde_json::{json, Value};

use gpui_kit::component::button::Button;
use gpui_kit::*;

use crate::extension::host::PanelSurface;
use crate::extension::ui::{
    ButtonStyle, NotificationLevel, PanelLocation, ThemePalette, UiActionRequest, UiDocument,
    UiNode, UiSession,
};
use crate::theme;

use super::{icon, txt, IconName};

/// The host palette with a panel's theme tokens layered over it.
struct Palette {
    page: u32,
    text: u32,
    muted: u32,
    faint: u32,
    border: u32,
    border_soft: u32,
    accent: u32,
    error: u32,
    success: u32,
    warn: u32,
    new_session_fill: u32,
    hover: u32,
}

impl Palette {
    fn new(theme: Option<&ThemePalette>) -> Self {
        let mut palette = Self {
            page: theme::page(),
            text: theme::text(),
            muted: theme::muted(),
            faint: theme::faint(),
            border: theme::border(),
            border_soft: theme::border_soft(),
            accent: theme::accent(),
            error: theme::error(),
            success: theme::success(),
            warn: theme::warn(),
            new_session_fill: theme::new_session_fill(),
            hover: theme::hover(),
        };
        if let Some(theme) = theme {
            for (token, slot) in [
                ("page", &mut palette.page),
                ("text", &mut palette.text),
                ("muted", &mut palette.muted),
                ("faint", &mut palette.faint),
                ("border", &mut palette.border),
                ("border-soft", &mut palette.border_soft),
                ("accent", &mut palette.accent),
                ("error", &mut palette.error),
                ("success", &mut palette.success),
                ("warn", &mut palette.warn),
                ("new-session-fill", &mut palette.new_session_fill),
                ("hover", &mut palette.hover),
            ] {
                if let Some(color) = theme.color(token) {
                    *slot = color;
                }
            }
        }
        palette
    }
}

/// One extension panel surface in the settings dialog.
pub(crate) struct ExtensionPanelView {
    panels: Vec<PanelSurface>,
    selected: usize,
    session: Option<UiSession>,
    document: Option<UiDocument>,
    loading: bool,
    error: Option<String>,
    message: Option<String>,
    palette: Palette,
}

impl ExtensionPanelView {
    /// Build a view over the panels the host currently offers.
    pub(crate) fn new(panels: Vec<PanelSurface>) -> Self {
        let selected = panels
            .iter()
            .position(|panel| {
                matches!(
                    panel.visibility,
                    crate::extension::ui::PanelVisibility::Visible
                )
            })
            .unwrap_or(0);
        let palette = panels
            .get(selected)
            .and_then(|panel| panel.theme.as_ref())
            .map(|theme| Palette::new(Some(theme)))
            .unwrap_or_else(|| Palette::new(None));
        let mut view = Self {
            panels,
            selected,
            session: None,
            document: None,
            loading: false,
            error: None,
            message: None,
            palette,
        };
        if let Some(panel) = view.panels.get(selected) {
            if let Some(document) = &panel.initial {
                match UiSession::open(panel.owner.clone(), document.clone()) {
                    Ok(session) => {
                        view.document = Some(session.document().clone());
                        view.session = Some(session);
                    }
                    Err(error) => view.error = Some(error.to_string()),
                }
            }
        }
        view
    }

    /// Replace the panel list, keeping a selection when the id still exists.
    pub(crate) fn set_panels(&mut self, panels: Vec<PanelSurface>) {
        let selected_id = self
            .panels
            .get(self.selected)
            .map(|panel| panel.panel_id.clone());
        self.panels = panels;
        self.selected = selected_id
            .and_then(|id| self.panels.iter().position(|panel| panel.panel_id == id))
            .unwrap_or(0);
        self.session = None;
        self.document = None;
        self.error = None;
        self.message = None;
        self.loading = false;
        let theme = self
            .panels
            .get(self.selected)
            .and_then(|panel| panel.theme.as_ref());
        self.palette = Palette::new(theme);
        if let Some(panel) = self.panels.get(self.selected) {
            if let Some(document) = &panel.initial {
                if let Ok(session) = UiSession::open(panel.owner.clone(), document.clone()) {
                    self.document = Some(session.document().clone());
                    self.session = Some(session);
                }
            }
        }
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.panels.len() || index == self.selected {
            return;
        }
        self.selected = index;
        self.session = None;
        self.document = None;
        self.error = None;
        self.message = None;
        self.loading = false;
        let theme = self.panels[index].theme.as_ref();
        self.palette = Palette::new(theme);
        if let Some(document) = self.panels[index].initial.clone() {
            match UiSession::open(self.panels[index].owner.clone(), document) {
                Ok(session) => {
                    self.document = Some(session.document().clone());
                    self.session = Some(session);
                }
                Err(error) => self.error = Some(error.to_string()),
            }
        } else {
            self.load(cx);
        }
        cx.notify();
    }

    fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if self.document.is_none() && !self.loading && self.error.is_none() {
            self.load(cx);
        }
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.panels.get(self.selected).cloned() else {
            return;
        };
        self.loading = true;
        self.error = None;
        self.message = None;
        cx.spawn(async move |this, cx| {
            let outcome = panel.runtime.panel(&panel.contribution_id).await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match outcome {
                    Ok(value) => match serde_json::from_value::<UiDocument>(value) {
                        Ok(document) => match UiSession::open(panel.owner.clone(), document) {
                            Ok(session) => {
                                this.document = Some(session.document().clone());
                                this.session = Some(session);
                                this.error = None;
                            }
                            Err(error) => this.error = Some(error.to_string()),
                        },
                        Err(error) => {
                            this.error = Some(format!("panel document is not valid: {error}"))
                        }
                    },
                    Err(error) => this.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn dispatch(&mut self, action: &str, payload: Value, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            return;
        };
        let request = UiActionRequest {
            action: action.to_string(),
            revision: session.revision(),
            payload: payload.clone(),
        };
        if let Err(error) = session.accept_action(&request) {
            self.error = Some(error.to_string());
            cx.notify();
            return;
        }
        let Some(panel) = self.panels.get(self.selected).cloned() else {
            return;
        };
        let revision = session.revision();
        let action = action.to_string();
        self.message = Some("Running…".to_string());
        cx.spawn(async move |this, cx| {
            let outcome = panel.runtime.command(&action, payload).await;
            let _ = this.update(cx, |this, cx| {
                this.message = None;
                match outcome {
                    Ok(value) => {
                        if let Ok(document) = serde_json::from_value::<UiDocument>(value.clone()) {
                            let Some(session) = this.session.as_mut() else {
                                return;
                            };
                            match session.apply(document, revision + 1) {
                                Ok(()) => {
                                    this.document = Some(session.document().clone());
                                    this.error = None;
                                }
                                Err(error) => this.error = Some(error.to_string()),
                            }
                        } else {
                            this.message = Some(summarize(&value));
                        }
                    }
                    Err(error) => this.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_node(&self, node: &UiNode, path: &str, cx: &mut Context<Self>) -> AnyElement {
        match node {
            UiNode::Container { children } | UiNode::Column { children } => {
                let mut column = div().flex().flex_col().gap(px(8.0)).w_full();
                for (index, child) in children.iter().enumerate() {
                    column = column.child(self.render_node(child, &format!("{path}-{index}"), cx));
                }
                column.into_any_element()
            }
            UiNode::Row { children } => {
                let mut row = div().flex().flex_row().items_center().gap(px(8.0)).w_full();
                for (index, child) in children.iter().enumerate() {
                    row = row.child(self.render_node(child, &format!("{path}-{index}"), cx));
                }
                row.into_any_element()
            }
            UiNode::Text { text } => txt(text.clone(), 13.0, self.palette.text).into_any_element(),
            UiNode::Heading { text, level } => {
                let size = match level {
                    1 => 21.0,
                    2 => 19.0,
                    3 => 18.0,
                    4 => 16.0,
                    _ => 14.0,
                };
                txt(text.clone(), size, self.palette.text)
                    .font_weight(FontWeight::SEMIBOLD)
                    .into_any_element()
            }
            UiNode::Markdown { text } => {
                txt(text.clone(), 13.0, self.palette.text).into_any_element()
            }
            UiNode::Button {
                label,
                action,
                style,
            } => {
                let (fill, ink) = match style {
                    ButtonStyle::Primary => (self.palette.accent, self.palette.page),
                    ButtonStyle::Danger => (self.palette.error, self.palette.page),
                    ButtonStyle::Ghost => (self.palette.page, self.palette.muted),
                    ButtonStyle::Secondary => (self.palette.new_session_fill, self.palette.text),
                };
                let action = action.clone();
                Button::new(format!("extension-action-{path}"))
                    .debug_selector(|| format!("extension-action-{path}"))
                    .accessibility_label(label.clone())
                    .px(px(12.0))
                    .h(px(30.0))
                    .rounded(px(8.0))
                    .bg(rgb(fill))
                    .border_1()
                    .border_color(rgb(self.palette.border_soft))
                    .child(txt(label.clone(), 13.0, ink))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.dispatch(&action, json!({}), cx)
                    }))
                    .into_any_element()
            }
            UiNode::Input {
                label,
                value,
                placeholder,
                action,
            } => {
                let action = action.clone();
                let current = if value.is_empty() { placeholder } else { value };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(txt(label.clone(), 12.0, self.palette.muted))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(30.0))
                                    .px(px(10.0))
                                    .flex()
                                    .items_center()
                                    .rounded(px(8.0))
                                    .bg(rgb(self.palette.page))
                                    .border_1()
                                    .border_color(rgb(self.palette.border))
                                    .child(txt(current.clone(), 13.0, self.palette.text)),
                            )
                            .child(
                                div()
                                    .id(format!("extension-apply-{path}"))
                                    .role(Role::Button)
                                    .aria_label(label.clone())
                                    .px(px(10.0))
                                    .h(px(30.0))
                                    .flex()
                                    .items_center()
                                    .rounded(px(8.0))
                                    .bg(rgb(self.palette.accent))
                                    .cursor_pointer()
                                    .child(txt("Apply", 12.0, self.palette.page))
                                    .on_click(cx.listener(move |this, _event, _window, cx| {
                                        this.dispatch(&action, json!({}), cx)
                                    })),
                            ),
                    )
                    .into_any_element()
            }
            UiNode::Select {
                label,
                value,
                options,
                action,
            } => {
                let action = action.clone();
                let mut row = div().flex().flex_col().gap(px(4.0)).child(txt(
                    if value.is_empty() {
                        label.clone()
                    } else {
                        format!("{label}: {value}")
                    },
                    12.0,
                    self.palette.muted,
                ));
                let mut chips = div().flex().flex_row().flex_wrap().gap(px(6.0));
                for option in options {
                    let action = action.clone();
                    let option_value = option.value.clone();
                    chips = chips.child(
                        div()
                            .id(format!("extension-option-{path}-{}", option.value))
                            .role(Role::Button)
                            .aria_label(option.label.clone())
                            .px(px(9.0))
                            .h(px(26.0))
                            .flex()
                            .items_center()
                            .rounded(px(7.0))
                            .bg(rgb(self.palette.new_session_fill))
                            .cursor_pointer()
                            .child(txt(option.label.clone(), 12.0, self.palette.text))
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.dispatch(&action, json!({ "value": option_value.clone() }), cx)
                            })),
                    );
                }
                row = row.child(chips);
                row.into_any_element()
            }
            UiNode::Checkbox {
                label,
                checked,
                action,
            } => {
                let action = action.clone();
                let next = !*checked;
                div()
                    .id(format!("extension-check-{path}"))
                    .role(Role::CheckBox)
                    .aria_label(label.clone())
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .cursor_pointer()
                    .child(if *checked {
                        icon(IconName::Check, 14.0, self.palette.accent).into_any_element()
                    } else {
                        div()
                            .w(px(14.0))
                            .h(px(14.0))
                            .rounded(px(3.0))
                            .border_1()
                            .border_color(rgb(self.palette.border))
                            .into_any_element()
                    })
                    .child(txt(label.clone(), 13.0, self.palette.text))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.dispatch(&action, json!({ "checked": next }), cx)
                    }))
                    .into_any_element()
            }
            UiNode::List { items } => {
                let mut list = div().flex().flex_col().gap(px(2.0)).w_full();
                for (index, item) in items.iter().enumerate() {
                    let mut row = div()
                        .id(SharedString::from(format!("extension-list-{path}-{index}")))
                        .w_full()
                        .px(px(8.0))
                        .py(px(6.0))
                        .rounded(px(7.0))
                        .flex()
                        .flex_col()
                        .hover(|style| style.bg(rgb(self.palette.hover)));
                    if let Some(action) = &item.action {
                        let action = action.clone();
                        row = row
                            .role(Role::Button)
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.dispatch(&action, json!({ "index": index }), cx)
                            }));
                    }
                    row = row.child(txt(item.title.clone(), 13.0, self.palette.text));
                    if !item.detail.is_empty() {
                        row = row.child(txt(item.detail.clone(), 11.0, self.palette.faint));
                    }
                    list = list.child(row);
                }
                list.into_any_element()
            }
            UiNode::Table { columns, rows } => {
                let mut table = div().flex().flex_col().gap(px(2.0)).w_full();
                let mut header = div().flex().flex_row().gap(px(8.0)).w_full();
                for column in columns {
                    header = header.child(div().flex_1().child(txt(
                        column.clone(),
                        12.0,
                        self.palette.muted,
                    )));
                }
                table = table.child(header);
                for row in rows {
                    let mut line = div().flex().flex_row().gap(px(8.0)).w_full();
                    for cell in row {
                        line = line.child(div().flex_1().child(txt(
                            cell.clone(),
                            12.0,
                            self.palette.text,
                        )));
                    }
                    table = table.child(line);
                }
                table.into_any_element()
            }
            UiNode::Tree { nodes } => {
                let mut tree = div().flex().flex_col().gap(px(2.0));
                for node in nodes {
                    tree = tree.child(self.render_tree(node, path, 0));
                }
                tree.into_any_element()
            }
            UiNode::Tabs { tabs } => {
                let mut body = div().flex().flex_col().gap(px(8.0)).w_full();
                let mut strip = div().flex().flex_row().gap(px(12.0));
                for tab in tabs {
                    strip = strip.child(txt(tab.label.clone(), 12.0, self.palette.muted));
                }
                body = body.child(strip);
                if let Some(first) = tabs.first() {
                    body = body.child(self.render_node(&first.content, &format!("{path}-tab"), cx));
                }
                body.into_any_element()
            }
            UiNode::Progress { value, label } => {
                let fraction: f32 = (*value).clamp(0.0, 1.0);
                let width = (220.0 * fraction).max(0.0);
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(txt(label.clone(), 12.0, self.palette.muted))
                    .child(
                        div()
                            .w(px(220.0))
                            .h(px(6.0))
                            .rounded(px(3.0))
                            .bg(rgb(self.palette.new_session_fill))
                            .child(
                                div()
                                    .w(px(width))
                                    .h(px(6.0))
                                    .rounded(px(3.0))
                                    .bg(rgb(self.palette.accent)),
                            ),
                    )
                    .into_any_element()
            }
            UiNode::Code { language, value } => {
                let mut code = div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .p(px(8.0))
                    .rounded(px(7.0))
                    .bg(rgb(self.palette.new_session_fill));
                if !language.is_empty() {
                    code = code.child(txt(language.clone(), 11.0, self.palette.faint));
                }
                code.child(txt(value.clone(), 12.0, self.palette.text))
                    .into_any_element()
            }
            UiNode::Image { src, alt } => div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(txt(
                    if alt.is_empty() {
                        src.clone()
                    } else {
                        alt.clone()
                    },
                    12.0,
                    self.palette.muted,
                ))
                .child(txt(src.clone(), 11.0, self.palette.faint))
                .into_any_element(),
            UiNode::Notification { level, text } => {
                let color = match level {
                    NotificationLevel::Info => self.palette.accent,
                    NotificationLevel::Warning => self.palette.warn,
                    NotificationLevel::Error => self.palette.error,
                    NotificationLevel::Success => self.palette.success,
                };
                txt(text.clone(), 12.0, color).into_any_element()
            }
            UiNode::Divider => div()
                .h(px(1.0))
                .w_full()
                .bg(rgb(self.palette.border_soft))
                .into_any_element(),
            UiNode::Spacer { size } => div().h(px(*size as f32)).into_any_element(),
        }
    }

    fn render_tree(
        &self,
        node: &crate::extension::ui::UiTreeNode,
        path: &str,
        depth: usize,
    ) -> AnyElement {
        let mut column = div().flex().flex_col().gap(px(2.0));
        column = column.child(div().pl(px(depth as f32 * 12.0)).child(txt(
            node.label.clone(),
            12.0,
            self.palette.text,
        )));
        for (index, child) in node.children.iter().enumerate() {
            column =
                column.child(self.render_tree(child, &format!("{path}-tree-{index}"), depth + 1));
        }
        column.into_any_element()
    }
}

fn summarize(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
    .chars()
    .take(4096)
    .collect()
}

impl Render for ExtensionPanelView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_loaded(cx);
        let mut root = div().flex().flex_col().gap(px(12.0)).w_full().h_full();
        if self.panels.is_empty() {
            return root.child(txt(
                "No extension panels are installed.",
                13.0,
                self.palette.muted,
            ));
        }
        let mut tabs = div().flex().flex_row().flex_wrap().gap(px(6.0));
        for (index, panel) in self.panels.iter().enumerate() {
            let selected = index == self.selected;
            let location = match panel.location {
                PanelLocation::Right => "right",
                PanelLocation::Left => "left",
                PanelLocation::Bottom => "bottom",
                PanelLocation::Center => "center",
            };
            tabs = tabs.child(
                div()
                    .id(format!("extension-panel-{}", panel.panel_id))
                    .debug_selector(|| format!("extension-panel-{}", panel.panel_id))
                    .role(Role::Button)
                    .aria_label(panel.title.clone())
                    .px(px(10.0))
                    .h(px(28.0))
                    .flex()
                    .items_center()
                    .rounded(px(7.0))
                    .bg(rgb(if selected {
                        self.palette.accent
                    } else {
                        self.palette.new_session_fill
                    }))
                    .cursor_pointer()
                    .child(txt(
                        format!("{} · {location}", panel.title),
                        12.0,
                        if selected {
                            self.palette.page
                        } else {
                            self.palette.text
                        },
                    ))
                    .on_click(cx.listener(move |this, _event, _window, cx| this.select(index, cx))),
            );
        }
        root = root.child(tabs);
        if self.loading {
            root = root.child(txt("Loading panel…", 12.0, self.palette.muted));
        }
        if let Some(error) = &self.error {
            root = root.child(txt(
                format!("Panel error: {error}"),
                12.0,
                self.palette.error,
            ));
        }
        if let Some(message) = &self.message {
            root = root.child(txt(message.clone(), 12.0, self.palette.muted));
        }
        if let Some(document) = &self.document {
            let mut body = div()
                .id("extension-panel-body")
                .flex()
                .flex_col()
                .gap(px(8.0))
                .w_full()
                .overflow_y_scroll();
            body = body.child(self.render_node(&document.root, "root", cx));
            root = root.child(body);
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use super::ExtensionPanelView;
    use crate::extension::host::PanelSurface;
    use crate::extension::runtime::{CallContext, Runtime, ToolDescriptor};
    use crate::extension::types::{ExtensionError, ExtensionId, RuntimeKind};
    use crate::extension::ui::{PanelLocation, ThemePalette, UiDocument};
    use crate::theme;
    use async_trait::async_trait;
    use serde_json::Value;
    use std::sync::Arc;

    struct Noop;

    #[async_trait]
    impl Runtime for Noop {
        fn kind(&self) -> RuntimeKind {
            RuntimeKind::Declarative
        }
        async fn tools(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
            Ok(Vec::new())
        }
        async fn call_tool(
            &self,
            _name: &str,
            _arguments: Value,
            _context: &CallContext,
        ) -> Result<Value, ExtensionError> {
            Ok(Value::Null)
        }
    }

    fn panel(document: UiDocument) -> PanelSurface {
        PanelSurface {
            panel_id: "com.example.ui.greeting".to_string(),
            contribution_id: "greeting".to_string(),
            title: "Greeting".to_string(),
            icon: None,
            location: PanelLocation::Right,
            visibility: crate::extension::ui::PanelVisibility::Visible,
            runtime: Arc::new(Noop),
            owner: ExtensionId::parse("com.example.ui").expect("id"),
            theme: None,
            initial: Some(document),
        }
    }

    #[test]
    fn an_initial_document_is_validated_before_it_is_shown() {
        let document: UiDocument = serde_json::from_value(serde_json::json!({
            "schema_version": "1.0",
            "revision": 1,
            "root": {
                "type": "column",
                "children": [
                    {"type": "heading", "text": "Hello", "level": 1},
                    {"type": "text", "text": "From an extension"}
                ]
            }
        }))
        .expect("document");
        let view = ExtensionPanelView::new(vec![panel(document)]);
        assert!(view.session.is_some());
        assert!(view.document.is_some());
        assert!(view.error.is_none());
        assert_eq!(view.panels.len(), 1);
    }

    #[test]
    fn a_panel_theme_overrides_the_host_palette() {
        let document: UiDocument = serde_json::from_value(serde_json::json!({
            "schema_version": "1.0",
            "revision": 1,
            "root": {"type": "text", "text": "Themed"}
        }))
        .expect("document");
        let mut surface = panel(document);
        let mut colors = std::collections::BTreeMap::new();
        colors.insert("page".to_string(), "#112233".to_string());
        colors.insert("accent".to_string(), "#445566".to_string());
        surface.theme = Some(ThemePalette {
            name: "Test theme".to_string(),
            colors,
        });
        let view = ExtensionPanelView::new(vec![surface]);
        assert_eq!(view.palette.page, 0x112233);
        assert_eq!(view.palette.accent, 0x445566);
        assert_eq!(view.palette.text, theme::text());
    }
}
