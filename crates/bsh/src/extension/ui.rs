//! The declarative UI contract.
//!
//! A package returns a bounded tree of components; the host renders it with its
//! own widgets and owns focus, layout and accessibility. No GPUI type crosses
//! this boundary, no HTML or script is accepted, and every tree is validated
//! against depth, node-count, text and payload ceilings before it reaches a
//! renderer. The revision number is what lets a host reject a patch produced by
//! a stale view.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::manifest::validate_relative_path;
use super::types::{ContributionId, ExtensionError, ExtensionId, ValidationReport};

/// The declarative UI schema generation.
pub(crate) const UI_SCHEMA_VERSION: &str = "1.0";

/// The largest serialized UI document this host accepts.
pub(crate) const MAX_UI_BYTES: usize = 1024 * 1024;

/// The structural ceilings applied to every document.
#[derive(Debug, Clone, Copy)]
pub(crate) struct UiLimits {
    /// Maximum nesting depth.
    pub(crate) max_depth: usize,
    /// Maximum node count.
    pub(crate) max_nodes: usize,
    /// Maximum characters in one text leaf.
    pub(crate) max_text: usize,
    /// Maximum characters across all text leaves.
    pub(crate) max_total_text: usize,
    /// Maximum list items.
    pub(crate) max_list_items: usize,
    /// Maximum table rows.
    pub(crate) max_table_rows: usize,
    /// Maximum table columns.
    pub(crate) max_columns: usize,
    /// Maximum characters in one table cell.
    pub(crate) max_cell: usize,
    /// Maximum tabs.
    pub(crate) max_tabs: usize,
}

impl Default for UiLimits {
    fn default() -> Self {
        Self {
            max_depth: 32,
            max_nodes: 1000,
            max_text: 16 * 1024,
            max_total_text: 256 * 1024,
            max_list_items: 500,
            max_table_rows: 500,
            max_columns: 32,
            max_cell: 2048,
            max_tabs: 16,
        }
    }
}

/// A theme contribution's colour tokens.
///
/// Values are CSS-style hex strings. The host validates the file and the GPUI
/// renderer parses the tokens it knows, falling back to the host palette for
/// anything absent, so a partial theme is a refinement rather than a broken
/// frame.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ThemePalette {
    /// Optional display name.
    #[serde(default)]
    pub(crate) name: String,
    /// token name -> #rrggbb / #rrggbbaa.
    #[serde(default)]
    pub(crate) colors: std::collections::BTreeMap<String, String>,
}

impl ThemePalette {
    /// Parse one token as RGB, ignoring alpha.
    pub(crate) fn color(&self, token: &str) -> Option<u32> {
        let raw = self.colors.get(token)?.trim();
        let hex = raw.strip_prefix('#').unwrap_or(raw);
        if hex.len() != 6 && hex.len() != 8 {
            return None;
        }
        u32::from_str_radix(&hex[..6], 16).ok()
    }
}

/// Where the host mounts a panel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PanelLocation {
    /// The right-hand panel rail.
    #[default]
    Right,
    /// The left-hand sidebar.
    Left,
    /// A bottom dock.
    Bottom,
    /// The main content column.
    Center,
}

/// Whether a panel is shown by default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PanelVisibility {
    /// Shown when the host surface opens.
    #[default]
    Visible,
    /// Registered but collapsed.
    Hidden,
}

/// A button's visual weight.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ButtonStyle {
    /// The primary action.
    Primary,
    /// A secondary action.
    #[default]
    Secondary,
    /// A destructive action.
    Danger,
    /// A quiet inline action.
    Ghost,
}

/// A notification's severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum NotificationLevel {
    /// Informational.
    Info,
    /// A warning.
    Warning,
    /// An error.
    Error,
    /// A success message.
    Success,
}

/// One option in a select.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct UiOption {
    /// Value submitted with the action.
    pub(crate) value: String,
    /// Visible label.
    pub(crate) label: String,
}

/// One row in a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct UiListItem {
    /// Primary text.
    pub(crate) title: String,
    /// Secondary text.
    #[serde(default)]
    pub(crate) detail: String,
    /// Action dispatched when the row is activated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) action: Option<String>,
    /// Whether the row shows a checked mark.
    #[serde(default)]
    pub(crate) checked: bool,
}

/// One node in a tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct UiTreeNode {
    /// Stable node id.
    pub(crate) id: String,
    /// Visible label.
    pub(crate) label: String,
    /// Children.
    #[serde(default)]
    pub(crate) children: Vec<UiTreeNode>,
}

/// One tab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct UiTab {
    /// Stable tab id.
    pub(crate) id: String,
    /// Visible label.
    pub(crate) label: String,
    /// Tab body.
    pub(crate) content: Box<UiNode>,
}

/// One component in a declarative view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub(crate) enum UiNode {
    /// A plain container.
    Container {
        /// Children.
        #[serde(default)]
        children: Vec<UiNode>,
    },
    /// A horizontal stack.
    Row {
        /// Children.
        #[serde(default)]
        children: Vec<UiNode>,
    },
    /// A vertical stack.
    Column {
        /// Children.
        #[serde(default)]
        children: Vec<UiNode>,
    },
    /// Body text.
    Text {
        /// The text.
        text: String,
    },
    /// A section heading.
    Heading {
        /// The heading text.
        text: String,
        /// Level, 1 through 6.
        #[serde(default = "default_heading_level")]
        level: u8,
    },
    /// Markdown text.
    Markdown {
        /// The document.
        text: String,
    },
    /// A button that dispatches an action.
    Button {
        /// Visible label.
        label: String,
        /// Action id.
        action: String,
        /// Visual weight.
        #[serde(default)]
        style: ButtonStyle,
    },
    /// A single-line text field.
    Input {
        /// Field label.
        label: String,
        /// Current value.
        #[serde(default)]
        value: String,
        /// Placeholder.
        #[serde(default)]
        placeholder: String,
        /// Action dispatched on submit.
        action: String,
    },
    /// A select field.
    Select {
        /// Field label.
        label: String,
        /// Current value.
        #[serde(default)]
        value: String,
        /// Options.
        options: Vec<UiOption>,
        /// Action dispatched on change.
        action: String,
    },
    /// A checkbox.
    Checkbox {
        /// Field label.
        label: String,
        /// Current state.
        #[serde(default)]
        checked: bool,
        /// Action dispatched on toggle.
        action: String,
    },
    /// A list.
    List {
        /// Rows.
        #[serde(default)]
        items: Vec<UiListItem>,
    },
    /// A table.
    Table {
        /// Column headers.
        columns: Vec<String>,
        /// Rows of cells.
        #[serde(default)]
        rows: Vec<Vec<String>>,
    },
    /// A tree.
    Tree {
        /// Roots.
        #[serde(default)]
        nodes: Vec<UiTreeNode>,
    },
    /// A tab strip.
    Tabs {
        /// Tabs.
        tabs: Vec<UiTab>,
    },
    /// A progress bar.
    Progress {
        /// Fraction from 0.0 to 1.0.
        #[serde(default)]
        value: f32,
        /// Optional label.
        #[serde(default)]
        label: String,
    },
    /// A code block.
    Code {
        /// Language name.
        #[serde(default)]
        language: String,
        /// Source text.
        value: String,
    },
    /// An image.
    Image {
        /// An https URL or an `asset/` package-relative path.
        src: String,
        /// Alternative text.
        #[serde(default)]
        alt: String,
    },
    /// An inline notification.
    Notification {
        /// Severity.
        level: NotificationLevel,
        /// Message.
        text: String,
    },
    /// A horizontal rule.
    Divider,
    /// Vertical space.
    Spacer {
        /// Height in device-independent pixels.
        #[serde(default = "default_spacer")]
        size: u16,
    },
}

fn default_heading_level() -> u8 {
    2
}

fn default_spacer() -> u16 {
    8
}

/// A complete declarative view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct UiDocument {
    /// Schema generation.
    pub(crate) schema_version: String,
    /// Root component.
    pub(crate) root: UiNode,
}

impl UiDocument {
    /// Validate the tree against the host limits and the owning extension.
    pub(crate) fn validate(&self, extension: &ExtensionId) -> ValidationReport {
        self.validate_with(extension, UiLimits::default())
    }

    /// Validate with explicit limits.
    pub(crate) fn validate_with(
        &self,
        _extension: &ExtensionId,
        limits: UiLimits,
    ) -> ValidationReport {
        let mut report = ValidationReport::default();
        if self.schema_version != UI_SCHEMA_VERSION {
            report.error(
                "ui.schema_version",
                format!(
                    "unsupported UI schema {:?}; this host reads {:?}",
                    self.schema_version, UI_SCHEMA_VERSION
                ),
            );
        }
        let mut state = WalkState {
            nodes: 0,
            text: 0,
            limits,
            report: &mut report,
        };
        walk(&self.root, 1, &mut state);
        match serde_json::to_vec(self) {
            Ok(bytes) if bytes.len() > MAX_UI_BYTES => report.error(
                "ui",
                format!(
                    "document is {} bytes; the ceiling is {MAX_UI_BYTES}",
                    bytes.len()
                ),
            ),
            Err(error) => report.error("ui", format!("document cannot be serialized: {error}")),
            Ok(_) => {}
        }
        report
    }
}

struct WalkState<'a> {
    nodes: usize,
    text: usize,
    limits: UiLimits,
    report: &'a mut ValidationReport,
}

fn walk(node: &UiNode, depth: usize, state: &mut WalkState<'_>) {
    state.nodes += 1;
    if state.nodes > state.limits.max_nodes {
        if state.nodes == state.limits.max_nodes + 1 {
            state.report.error(
                "ui",
                format!("document has more than {} nodes", state.limits.max_nodes),
            );
        }
        return;
    }
    if depth > state.limits.max_depth {
        state.report.error(
            "ui",
            format!(
                "document nests deeper than {} levels",
                state.limits.max_depth
            ),
        );
        return;
    }
    match node {
        UiNode::Container { children } | UiNode::Row { children } | UiNode::Column { children } => {
            for child in children {
                walk(child, depth + 1, state);
            }
        }
        UiNode::Text { text } | UiNode::Markdown { text } | UiNode::Notification { text, .. } => {
            text_leaf("ui.text", text, state);
        }
        UiNode::Heading { text, level } => {
            if !(1..=6).contains(level) {
                state
                    .report
                    .error("ui.heading.level", "must be 1 through 6");
            }
            text_leaf("ui.heading.text", text, state);
        }
        UiNode::Button { label, action, .. } => {
            text_leaf("ui.button.label", label, state);
            action_id("ui.button.action", action, state);
        }
        UiNode::Input {
            label,
            value,
            placeholder,
            action,
        } => {
            text_leaf("ui.input.label", label, state);
            text_leaf("ui.input.value", value, state);
            text_leaf("ui.input.placeholder", placeholder, state);
            action_id("ui.input.action", action, state);
        }
        UiNode::Select {
            label,
            value,
            options,
            action,
        } => {
            text_leaf("ui.select.label", label, state);
            text_leaf("ui.select.value", value, state);
            if options.len() > 200 {
                state
                    .report
                    .error("ui.select.options", "must contain at most 200 options");
            }
            for option in options {
                text_leaf("ui.select.option.label", &option.label, state);
                text_leaf("ui.select.option.value", &option.value, state);
            }
            action_id("ui.select.action", action, state);
        }
        UiNode::Checkbox {
            label,
            checked: _,
            action,
        } => {
            text_leaf("ui.checkbox.label", label, state);
            action_id("ui.checkbox.action", action, state);
        }
        UiNode::List { items } => {
            if items.len() > state.limits.max_list_items {
                state.report.error(
                    "ui.list.items",
                    format!("must contain at most {} items", state.limits.max_list_items),
                );
            }
            for item in items {
                text_leaf("ui.list.item.title", &item.title, state);
                text_leaf("ui.list.item.detail", &item.detail, state);
                if let Some(action) = &item.action {
                    action_id("ui.list.item.action", action, state);
                }
            }
        }
        UiNode::Table { columns, rows } => {
            if columns.len() > state.limits.max_columns {
                state.report.error(
                    "ui.table.columns",
                    format!("must have at most {} columns", state.limits.max_columns),
                );
            }
            if rows.len() > state.limits.max_table_rows {
                state.report.error(
                    "ui.table.rows",
                    format!("must have at most {} rows", state.limits.max_table_rows),
                );
            }
            for column in columns {
                text_leaf("ui.table.column", column, state);
            }
            for row in rows {
                if row.len() != columns.len() {
                    state.report.error(
                        "ui.table.row",
                        "every row must have exactly one cell per column",
                    );
                }
                for cell in row {
                    if cell.chars().count() > state.limits.max_cell {
                        state.report.error(
                            "ui.table.cell",
                            format!("must be at most {} characters", state.limits.max_cell),
                        );
                    }
                    text_leaf("ui.table.cell", cell, state);
                }
            }
        }
        UiNode::Tree { nodes } => {
            for node in nodes {
                walk_tree(node, depth + 1, state);
            }
        }
        UiNode::Tabs { tabs } => {
            if tabs.len() > state.limits.max_tabs {
                state.report.error(
                    "ui.tabs",
                    format!("must contain at most {} tabs", state.limits.max_tabs),
                );
            }
            let mut seen = std::collections::BTreeSet::new();
            for tab in tabs {
                if tab.id.is_empty() || !seen.insert(tab.id.clone()) {
                    state
                        .report
                        .error("ui.tabs.id", "tab ids must be non-empty and unique");
                }
                text_leaf("ui.tabs.label", &tab.label, state);
                walk(&tab.content, depth + 1, state);
            }
        }
        UiNode::Progress { value, label } => {
            if !(0.0..=1.0).contains(value) {
                state
                    .report
                    .error("ui.progress.value", "must be between 0.0 and 1.0");
            }
            text_leaf("ui.progress.label", label, state);
        }
        UiNode::Code { language, value } => {
            text_leaf("ui.code.language", language, state);
            text_leaf("ui.code.value", value, state);
        }
        UiNode::Image { src, alt } => {
            if !valid_image_source(src) {
                state.report.error(
                    "ui.image.src",
                    "must be an https:// URL or an asset/ package-relative path",
                );
            }
            text_leaf("ui.image.alt", alt, state);
        }
        UiNode::Divider => {}
        UiNode::Spacer { size } => {
            if *size > 512 {
                state.report.error("ui.spacer.size", "must be at most 512");
            }
        }
    }
}

fn walk_tree(node: &UiTreeNode, depth: usize, state: &mut WalkState<'_>) {
    state.nodes += 1;
    if state.nodes > state.limits.max_nodes {
        if state.nodes == state.limits.max_nodes + 1 {
            state.report.error(
                "ui",
                format!("document has more than {} nodes", state.limits.max_nodes),
            );
        }
        return;
    }
    if depth > state.limits.max_depth {
        state.report.error(
            "ui.tree",
            format!("tree nests deeper than {} levels", state.limits.max_depth),
        );
        return;
    }
    if node.id.is_empty() {
        state
            .report
            .error("ui.tree.id", "node ids must not be empty");
    }
    text_leaf("ui.tree.label", &node.label, state);
    for child in &node.children {
        walk_tree(child, depth + 1, state);
    }
}

fn text_leaf(field: &str, text: &str, state: &mut WalkState<'_>) {
    let characters = text.chars().count();
    if characters > state.limits.max_text {
        state.report.error(
            field,
            format!("must be at most {} characters", state.limits.max_text),
        );
    }
    state.text += characters;
    if state.text > state.limits.max_total_text {
        state.report.error(
            "ui",
            format!(
                "document carries more than {} characters of text",
                state.limits.max_total_text
            ),
        );
    }
}

fn action_id(field: &str, action: &str, state: &mut WalkState<'_>) {
    if ContributionId::parse(action).is_err() {
        state.report.error(
            field,
            "must be a dotted contribution id owned by the extension",
        );
    }
}

fn valid_image_source(src: &str) -> bool {
    if let Some(rest) = src.strip_prefix("https://") {
        return !rest.is_empty() && !rest.contains(char::is_whitespace);
    }
    if let Some(rest) = src.strip_prefix("asset/") {
        return validate_relative_path(rest).is_ok();
    }
    false
}

/// One UI action request from the host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct UiActionRequest {
    /// The action id the control carried.
    pub(crate) action: String,
    /// The revision the control was rendered from.
    pub(crate) revision: u64,
    /// Action payload.
    #[serde(default)]
    pub(crate) payload: Value,
}

/// One extension's live declarative view.
#[derive(Debug, Clone)]
pub(crate) struct UiSession {
    extension: ExtensionId,
    revision: u64,
    document: UiDocument,
}

impl UiSession {
    /// Start a session from a validated document.
    pub(crate) fn open(
        extension: ExtensionId,
        document: UiDocument,
    ) -> Result<Self, ExtensionError> {
        let report = document.validate(&extension);
        if !report.is_valid() {
            return Err(ExtensionError::Manifest {
                path: std::path::PathBuf::from("<ui>"),
                message: report
                    .errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; "),
            });
        }
        Ok(Self {
            extension,
            revision: 1,
            document,
        })
    }

    /// The current revision.
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    /// The current document.
    pub(crate) fn document(&self) -> &UiDocument {
        &self.document
    }

    /// Apply a new document at `revision`. A revision at or below the current
    /// one is stale and refused.
    pub(crate) fn apply(
        &mut self,
        document: UiDocument,
        revision: u64,
    ) -> Result<(), ExtensionError> {
        if revision <= self.revision {
            return Err(ExtensionError::Protocol {
                extension: self.extension.to_string(),
                message: format!(
                    "stale UI patch at revision {revision}; current revision is {}",
                    self.revision
                ),
            });
        }
        let report = document.validate(&self.extension);
        if !report.is_valid() {
            return Err(ExtensionError::Manifest {
                path: std::path::PathBuf::from("<ui>"),
                message: report
                    .errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; "),
            });
        }
        self.document = document;
        self.revision = revision;
        Ok(())
    }

    /// Validate an action against the current revision and the action grammar.
    pub(crate) fn accept_action(
        &self,
        request: &UiActionRequest,
    ) -> Result<ContributionId, ExtensionError> {
        if request.revision != self.revision {
            return Err(ExtensionError::Protocol {
                extension: self.extension.to_string(),
                message: format!(
                    "action at revision {} but the view is at {}; the control was stale",
                    request.revision, self.revision
                ),
            });
        }
        ContributionId::parse(request.action.clone()).map_err(|_| ExtensionError::Protocol {
            extension: self.extension.to_string(),
            message: format!("action {:?} is not a contribution id", request.action),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> ExtensionId {
        ExtensionId::parse("com.example.ui").expect("id")
    }

    fn document() -> UiDocument {
        UiDocument {
            schema_version: UI_SCHEMA_VERSION.to_string(),
            root: UiNode::Column {
                children: vec![
                    UiNode::Heading {
                        text: "Git Status".to_string(),
                        level: 2,
                    },
                    UiNode::Text {
                        text: "Working tree clean".to_string(),
                    },
                    UiNode::Button {
                        label: "Refresh".to_string(),
                        action: "git.refresh".to_string(),
                        style: ButtonStyle::Primary,
                    },
                ],
            },
        }
    }

    #[test]
    fn a_valid_document_passes() {
        let report = document().validate(&id());
        assert!(report.is_valid(), "{report:?}");
    }

    #[test]
    fn a_bad_action_is_refused() {
        let mut document = document();
        if let UiNode::Column { children } = &mut document.root {
            if let UiNode::Button { action, .. } = &mut children[2] {
                *action = "Refresh!".to_string();
            }
        }
        let report = document.validate(&id());
        assert!(!report.is_valid());
        assert!(report
            .errors
            .iter()
            .any(|issue| issue.field == "ui.button.action"));
    }

    #[test]
    fn depth_is_capped() {
        let mut root = UiNode::Text {
            text: "leaf".to_string(),
        };
        for _ in 0..40 {
            root = UiNode::Column {
                children: vec![root],
            };
        }
        let report = UiDocument {
            schema_version: UI_SCHEMA_VERSION.to_string(),
            root,
        }
        .validate(&id());
        assert!(!report.is_valid());
    }

    #[test]
    fn node_count_is_capped() {
        let children: Vec<UiNode> = (0..1001).map(|_| UiNode::Divider).collect();
        let report = UiDocument {
            schema_version: UI_SCHEMA_VERSION.to_string(),
            root: UiNode::Column { children },
        }
        .validate(&id());
        assert!(!report.is_valid());
    }

    #[test]
    fn images_only_accept_https_or_package_assets() {
        for src in ["https://example.com/a.png", "asset/icons/a.png"] {
            assert!(valid_image_source(src), "{src}");
        }
        for src in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "data:image/png;base64,xxx",
            "asset/../escape.png",
            "http://example.com/a.png",
        ] {
            assert!(!valid_image_source(src), "{src}");
        }
    }

    #[test]
    fn a_stale_patch_or_action_is_refused() {
        let mut session = UiSession::open(id(), document()).expect("session");
        assert_eq!(session.revision(), 1);
        session.apply(document(), 2).expect("apply");
        assert!(session.apply(document(), 2).is_err());
        assert!(session.apply(document(), 1).is_err());
        let stale = UiActionRequest {
            action: "git.refresh".to_string(),
            revision: 1,
            payload: Value::Null,
        };
        assert!(session.accept_action(&stale).is_err());
        let current = UiActionRequest {
            action: "git.refresh".to_string(),
            revision: 2,
            payload: Value::Null,
        };
        assert!(session.accept_action(&current).is_ok());
    }
}
