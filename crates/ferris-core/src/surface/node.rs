//! The schema 1.1 surface node vocabulary.
//!
//! Schema 1.1 is a strict superset of the declarative schema 1.0: the same
//! component kinds, plus a stable optional node id, a slot node that composes
//! another surface, and a surface tab that can hold a slot.
//!
//! A schema 1.0 document lowers into this tree losslessly, without ids, so a
//! host can keep reading shipped panel files while the surface path lands.

use serde::{Deserialize, Serialize};

use crate::ui::{ButtonStyle, NotificationLevel, Tone, UiListItem, UiOption, UiTreeNode};

/// A node in a surface document.
///
/// Interactive nodes must carry an id. The id is the host key for focus,
/// selection, scroll and input-method state, and the address a patch uses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum SurfaceNode {
    /// A plain container.
    Container {
        /// Stable node id, when the producer needs one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// Children.
        #[serde(default)]
        children: Vec<SurfaceNode>,
    },
    /// A horizontal stack.
    Row {
        /// Stable node id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// Children.
        #[serde(default)]
        children: Vec<SurfaceNode>,
    },
    /// A vertical stack.
    Column {
        /// Stable node id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// Children.
        #[serde(default)]
        children: Vec<SurfaceNode>,
    },
    /// Body text.
    Text {
        /// Stable node id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// The text.
        text: String,
        /// Semantic colour role.
        #[serde(default)]
        tone: Tone,
    },
    /// A section heading.
    Heading {
        /// Stable node id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// The text.
        text: String,
        /// Level, 1 through 6.
        #[serde(default = "default_heading_level")]
        level: u8,
        /// Semantic colour role.
        #[serde(default)]
        tone: Tone,
    },
    /// Markdown text.
    Markdown {
        /// Stable node id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// The document.
        text: String,
    },
    /// A button that dispatches a binding.
    Button {
        /// Stable node id. Required: the host owns its focus and pending state.
        id: String,
        /// Visible label.
        label: String,
        /// Binding key from the document table.
        action: String,
        /// Visual weight.
        #[serde(default)]
        style: ButtonStyle,
    },
    /// A single-line text field.
    Input {
        /// Stable node id. Required.
        id: String,
        /// Field label.
        label: String,
        /// Current value.
        #[serde(default)]
        value: String,
        /// Placeholder.
        #[serde(default)]
        placeholder: String,
        /// Binding key dispatched on submit.
        action: String,
    },
    /// A select field.
    Select {
        /// Stable node id. Required.
        id: String,
        /// Field label.
        label: String,
        /// Current value.
        #[serde(default)]
        value: String,
        /// Options.
        options: Vec<UiOption>,
        /// Binding key dispatched on change.
        action: String,
    },
    /// A checkbox.
    Checkbox {
        /// Stable node id. Required.
        id: String,
        /// Field label.
        label: String,
        /// Current state.
        #[serde(default)]
        checked: bool,
        /// Binding key dispatched on toggle.
        action: String,
    },
    /// A list.
    List {
        /// Stable node id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// Rows.
        #[serde(default)]
        items: Vec<UiListItem>,
    },
    /// A table.
    Table {
        /// Stable node id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// Column headers.
        columns: Vec<String>,
        /// Rows of cells.
        #[serde(default)]
        rows: Vec<Vec<String>>,
        /// Relative column widths.
        #[serde(default)]
        widths: Vec<f32>,
    },
    /// A collapsible section.
    Section {
        /// Stable node id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// The disclosure header.
        title: String,
        /// Whether it starts open.
        #[serde(default = "section_starts_open")]
        open: bool,
        /// The body.
        #[serde(default)]
        children: Vec<SurfaceNode>,
    },
    /// A tree.
    Tree {
        /// Stable node id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// Roots.
        #[serde(default)]
        nodes: Vec<UiTreeNode>,
    },
    /// A tab strip.
    Tabs {
        /// Stable node id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// Tabs.
        tabs: Vec<SurfaceTab>,
    },
    /// A progress bar.
    Progress {
        /// Stable node id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// Fraction from 0.0 to 1.0.
        #[serde(default)]
        value: f32,
        /// Optional label.
        #[serde(default)]
        label: String,
    },
    /// A code block.
    Code {
        /// Stable node id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// Language name.
        #[serde(default)]
        language: String,
        /// Source text.
        value: String,
        /// Semantic colour role.
        #[serde(default)]
        tone: Tone,
    },
    /// An image from a package asset path.
    Image {
        /// Stable node id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// An asset package-relative path.
        src: String,
        /// Alternative text.
        #[serde(default)]
        alt: String,
    },
    /// An inline notification.
    Notification {
        /// Stable node id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// Severity.
        level: NotificationLevel,
        /// Message.
        text: String,
    },
    /// A horizontal rule.
    Divider {
        /// Stable node id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// Vertical space.
    Spacer {
        /// Stable node id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// Height in device-independent pixels.
        #[serde(default = "default_spacer")]
        size: u16,
    },
    /// A mount point for another surface.
    Slot {
        /// Stable node id, when the producer needs one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// Provider-qualified surface contract.
        surface: String,
        /// Accepted contract version range.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        /// Whether the region is Pending until a provider resolves.
        #[serde(default = "default_true")]
        required: bool,
        /// Bounded, schema-checked mount props.
        #[serde(default)]
        props: serde_json::Value,
        /// Host-owned sizing constraints.
        #[serde(default)]
        sizing: Sizing,
        /// Drawn only when an optional slot is unresolved.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fallback: Option<Box<SurfaceNode>>,
    },
}

/// A tab whose body can itself contain a slot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SurfaceTab {
    /// Stable tab id.
    pub id: String,
    /// Visible label.
    pub label: String,
    /// Tab body.
    pub content: Box<SurfaceNode>,
}

/// Host-owned sizing constraints for a slot region.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Sizing {
    /// Relative weight among siblings.
    #[serde(default = "default_weight")]
    pub weight: f32,
    /// Minimum size in device-independent pixels.
    #[serde(default)]
    pub min: u32,
    /// Maximum size; zero means unconstrained.
    #[serde(default)]
    pub max: u32,
    /// What the host does when content exceeds the region.
    #[serde(default)]
    pub overflow: Overflow,
}

impl Default for Sizing {
    fn default() -> Self {
        Self {
            weight: 1.0,
            min: 0,
            max: 0,
            overflow: Overflow::Scroll,
        }
    }
}

/// How a slot region handles overflow.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Overflow {
    /// The host scrolls the region.
    #[default]
    Scroll,
    /// The host clips the region.
    Clip,
    /// The host shrinks content to fit.
    Fit,
}

/// The node id, when the node carries one.
impl SurfaceNode {
    /// The stable id, when this node declares one.
    pub fn id(&self) -> Option<&str> {
        match self {
            Self::Container { id, .. }
            | Self::Row { id, .. }
            | Self::Column { id, .. }
            | Self::Text { id, .. }
            | Self::Heading { id, .. }
            | Self::Markdown { id, .. }
            | Self::List { id, .. }
            | Self::Table { id, .. }
            | Self::Section { id, .. }
            | Self::Tree { id, .. }
            | Self::Tabs { id, .. }
            | Self::Progress { id, .. }
            | Self::Code { id, .. }
            | Self::Image { id, .. }
            | Self::Notification { id, .. }
            | Self::Divider { id, .. }
            | Self::Spacer { id, .. }
            | Self::Slot { id, .. } => id.as_deref(),
            Self::Button { id, .. }
            | Self::Input { id, .. }
            | Self::Select { id, .. }
            | Self::Checkbox { id, .. } => Some(id),
        }
    }

    /// Whether this node is interactive and therefore must carry an id.
    pub fn is_interactive(&self) -> bool {
        matches!(
            self,
            Self::Button { .. } | Self::Input { .. } | Self::Select { .. } | Self::Checkbox { .. }
        )
    }

    /// Whether this node mounts another surface.
    pub fn is_slot(&self) -> bool {
        matches!(self, Self::Slot { .. })
    }

    /// The children of a container, section or row.
    ///
    /// A tab body is a child too, but it is reached through the tab because
    /// the host draws one body at a time. Validation walks both lists.
    pub fn container_children(&self) -> Option<&[SurfaceNode]> {
        match self {
            Self::Container { children, .. }
            | Self::Row { children, .. }
            | Self::Column { children, .. }
            | Self::Section { children, .. } => Some(children),
            _ => None,
        }
    }
}

fn default_heading_level() -> u8 {
    2
}

fn section_starts_open() -> bool {
    true
}

fn default_spacer() -> u16 {
    8
}

fn default_weight() -> f32 {
    1.0
}

fn default_true() -> bool {
    true
}

impl From<crate::ui::UiNode> for SurfaceNode {
    /// Lower a schema 1.0 node into schema 1.1.
    ///
    /// A schema 1.0 document has no node ids. Interactive nodes get a
    /// deterministic, unique id from a preorder counter, which is stable for
    /// identical input and never collides inside one document. The ids are an
    /// implementation detail of the compatibility path: a 1.0 producer never
    /// addresses them, and a host that renders the tree treats them as opaque.
    fn from(node: crate::ui::UiNode) -> Self {
        let mut counter = 0usize;
        surface_from_ui(node, &mut counter)
    }
}

fn surface_children(nodes: Vec<crate::ui::UiNode>, counter: &mut usize) -> Vec<SurfaceNode> {
    nodes
        .into_iter()
        .map(|child| surface_from_ui(child, counter))
        .collect()
}

fn next_control_id(counter: &mut usize, kind: &str) -> String {
    *counter += 1;
    format!("{kind}-{counter}")
}

fn surface_from_ui(node: crate::ui::UiNode, counter: &mut usize) -> SurfaceNode {
    use crate::ui::UiNode;
    match node {
        UiNode::Container { children } => SurfaceNode::Container {
            id: None,
            children: surface_children(children, counter),
        },
        UiNode::Row { children } => SurfaceNode::Row {
            id: None,
            children: surface_children(children, counter),
        },
        UiNode::Column { children } => SurfaceNode::Column {
            id: None,
            children: surface_children(children, counter),
        },
        UiNode::Text { text, tone } => SurfaceNode::Text {
            id: None,
            text,
            tone,
        },
        UiNode::Heading { text, level, tone } => SurfaceNode::Heading {
            id: None,
            text,
            level,
            tone,
        },
        UiNode::Markdown { text } => SurfaceNode::Markdown { id: None, text },
        UiNode::Button {
            label,
            action,
            style,
        } => SurfaceNode::Button {
            id: next_control_id(counter, "button"),
            label,
            action,
            style,
        },
        UiNode::Input {
            label,
            value,
            placeholder,
            action,
        } => SurfaceNode::Input {
            id: next_control_id(counter, "input"),
            label,
            value,
            placeholder,
            action,
        },
        UiNode::Select {
            label,
            value,
            options,
            action,
        } => SurfaceNode::Select {
            id: next_control_id(counter, "select"),
            label,
            value,
            options,
            action,
        },
        UiNode::Checkbox {
            label,
            checked,
            action,
        } => SurfaceNode::Checkbox {
            id: next_control_id(counter, "checkbox"),
            label,
            checked,
            action,
        },
        UiNode::List { items } => SurfaceNode::List { id: None, items },
        UiNode::Table {
            columns,
            rows,
            widths,
        } => SurfaceNode::Table {
            id: None,
            columns,
            rows,
            widths,
        },
        UiNode::Section {
            title,
            open,
            children,
        } => SurfaceNode::Section {
            id: None,
            title,
            open,
            children: surface_children(children, counter),
        },
        UiNode::Tree { nodes } => SurfaceNode::Tree { id: None, nodes },
        UiNode::Tabs { tabs } => SurfaceNode::Tabs {
            id: None,
            tabs: tabs
                .into_iter()
                .map(|tab| SurfaceTab {
                    id: tab.id,
                    label: tab.label,
                    content: Box::new(surface_from_ui(*tab.content, counter)),
                })
                .collect(),
        },
        UiNode::Progress { value, label } => SurfaceNode::Progress {
            id: None,
            value,
            label,
        },
        UiNode::Code {
            language,
            value,
            tone,
        } => SurfaceNode::Code {
            id: None,
            language,
            value,
            tone,
        },
        UiNode::Image { src, alt } => SurfaceNode::Image { id: None, src, alt },
        UiNode::Notification { level, text } => SurfaceNode::Notification {
            id: None,
            level,
            text,
        },
        UiNode::Divider => SurfaceNode::Divider { id: None },
        UiNode::Spacer { size } => SurfaceNode::Spacer { id: None, size },
    }
}

impl SurfaceNode {
    /// Lower this tree to the schema 1.0 vocabulary for the existing renderer.
    ///
    /// Node ids are dropped (1.0 nodes have none) and a slot lowers to its
    /// fallback, or an empty container when it has none. The host renderer
    /// reads the surface document directly for ownership; this lowering exists
    /// for a renderer that only knows the 1.0 node kinds.
    pub fn to_ui_node(&self) -> crate::ui::UiNode {
        let mut value = serde_json::to_value(self).expect("a surface node serializes");
        lower_slots(&mut value);
        serde_json::from_value(value).expect("the 1.0 vocabulary is a subset of 1.1")
    }
}

/// Replace every slot node in a serialized surface tree with its fallback.
fn lower_slots(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            if map.get("type").and_then(serde_json::Value::as_str) == Some("slot") {
                let fallback = map.remove("fallback");
                let mut replacement = fallback
                    .unwrap_or_else(|| serde_json::json!({"type": "container", "children": []}));
                lower_slots(&mut replacement);
                *value = replacement;
                return;
            }
            for child in map.values_mut() {
                lower_slots(child);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                lower_slots(item);
            }
        }
        _ => {}
    }
}
