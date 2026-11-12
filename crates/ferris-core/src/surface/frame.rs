//! Frames and base-relative patches.
//!
//! A frame is either a full document or a patch against the revision the
//! producer last observed. Patch application is atomic: a patch that fails
//! validation changes nothing, so a host never renders a half-applied tree.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::document::{ActionBinding, SurfaceDocument};
use super::node::SurfaceNode;
use super::{Generation, InstanceId, Revision, SurfaceError, SurfaceLimits};

/// One provider update.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SurfaceFrame {
    /// The instance the frame belongs to.
    pub instance: InstanceId,
    /// The generation the producer believes is current.
    pub generation: Generation,
    /// The revision this frame produces.
    pub revision: Revision,
    /// None for a full document; the base revision for a patch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_revision: Option<Revision>,
    /// The full document, when this is not a patch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<SurfaceDocument>,
    /// The patch operations, empty on a full frame.
    #[serde(default)]
    pub patch: Vec<PatchOp>,
    /// The provider hit its own budget and says the frame is partial.
    #[serde(default)]
    pub truncated: bool,
}

impl SurfaceFrame {
    /// A full-document frame.
    pub fn full(
        instance: InstanceId,
        generation: Generation,
        revision: Revision,
        document: SurfaceDocument,
    ) -> Self {
        Self {
            instance,
            generation,
            revision,
            base_revision: None,
            document: Some(document),
            patch: Vec::new(),
            truncated: false,
        }
    }

    /// A base-relative patch frame.
    pub fn patch(
        instance: InstanceId,
        generation: Generation,
        base: Revision,
        revision: Revision,
        patch: Vec<PatchOp>,
    ) -> Self {
        Self {
            instance,
            generation,
            revision,
            base_revision: Some(base),
            document: None,
            patch,
            truncated: false,
        }
    }
}

/// One patch operation. An empty node id addresses the document root.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PatchOp {
    /// Replace a node, or the root when the id is empty.
    Replace {
        /// Target node id, or an empty string for the root.
        id: String,
        /// The replacement node.
        node: SurfaceNode,
    },
    /// Insert a child into a container-like node.
    Insert {
        /// Parent node id; the root may not be inserted into.
        parent: String,
        /// Insertion index.
        index: u32,
        /// The new node.
        node: SurfaceNode,
    },
    /// Remove a node. The root cannot be removed.
    Remove {
        /// Target node id.
        id: String,
    },
    /// Replace the text of a text-like node.
    SetText {
        /// Target node id.
        id: String,
        /// The new text.
        text: String,
    },
    /// Replace the props of a slot node.
    SetProps {
        /// Target slot node id.
        id: String,
        /// The new props.
        props: Value,
    },
    /// Add, replace or remove action bindings.
    SetBindings {
        /// Binding key to its new value, or None to remove it.
        changed: BTreeMap<String, Option<ActionBinding>>,
    },
}

impl PatchOp {
    /// The node id this operation addresses, when it addresses one.
    pub fn target(&self) -> Option<&str> {
        match self {
            Self::Replace { id, .. }
            | Self::Remove { id }
            | Self::SetText { id, .. }
            | Self::SetProps { id, .. } => Some(id),
            Self::Insert { parent, .. } => Some(parent),
            Self::SetBindings { .. } => None,
        }
    }
}

/// Apply a patch to a document, then validate the result.
///
/// The input is not modified: a rejected patch leaves the caller with its
/// previous document, byte for byte.
pub fn apply_patch(
    document: &SurfaceDocument,
    patch: &[PatchOp],
    limits: &SurfaceLimits,
) -> Result<SurfaceDocument, SurfaceError> {
    if patch.len() > limits.max_patch_ops {
        return Err(SurfaceError::Budget(format!(
            "patch carries {} operations; the ceiling is {}",
            patch.len(),
            limits.max_patch_ops
        )));
    }
    let mut next = document.clone();
    for op in patch {
        match op {
            PatchOp::Replace { id, node } => {
                if id.is_empty() {
                    next.root = node.clone();
                } else {
                    let target = find_mut(&mut next.root, id)
                        .ok_or_else(|| SurfaceError::UnknownNode(id.clone()))?;
                    *target = node.clone();
                }
            }
            PatchOp::Insert {
                parent,
                index,
                node,
            } => {
                insert_child(&mut next.root, parent, *index as usize, node.clone())?;
            }
            PatchOp::Remove { id } => {
                if id.is_empty() {
                    return Err(SurfaceError::Invalid(
                        "the document root cannot be removed".to_string(),
                    ));
                }
                if !remove_child(&mut next.root, id) {
                    return Err(SurfaceError::UnknownNode(id.clone()));
                }
            }
            PatchOp::SetText { id, text } => {
                let target = find_mut(&mut next.root, id)
                    .ok_or_else(|| SurfaceError::UnknownNode(id.clone()))?;
                set_text(target, text)?;
            }
            PatchOp::SetProps { id, props } => {
                let target = find_mut(&mut next.root, id)
                    .ok_or_else(|| SurfaceError::UnknownNode(id.clone()))?;
                match target {
                    SurfaceNode::Slot { props: current, .. } => *current = props.clone(),
                    _ => {
                        return Err(SurfaceError::Invalid(format!(
                            "node {id:?} is not a slot and has no props"
                        )))
                    }
                }
            }
            PatchOp::SetBindings { changed } => {
                for (key, value) in changed {
                    match value {
                        Some(binding) => {
                            next.bindings.insert(key.clone(), binding.clone());
                        }
                        None => {
                            next.bindings.remove(key);
                        }
                    }
                }
            }
        }
    }
    let report = next.validate_with(limits);
    if !report.is_valid() {
        return Err(SurfaceError::Invalid(
            report
                .errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }
    Ok(next)
}

fn find_mut<'a>(node: &'a mut SurfaceNode, id: &str) -> Option<&'a mut SurfaceNode> {
    if node.id() == Some(id) {
        return Some(node);
    }
    match node {
        SurfaceNode::Container { children, .. }
        | SurfaceNode::Row { children, .. }
        | SurfaceNode::Column { children, .. }
        | SurfaceNode::Section { children, .. } => {
            for child in children {
                if let Some(found) = find_mut(child, id) {
                    return Some(found);
                }
            }
            None
        }
        SurfaceNode::Tabs { tabs, .. } => {
            for tab in tabs {
                if let Some(found) = find_mut(&mut tab.content, id) {
                    return Some(found);
                }
            }
            None
        }
        SurfaceNode::Slot { fallback, .. } => fallback
            .as_mut()
            .and_then(|fallback| find_mut(fallback, id)),
        _ => None,
    }
}

fn insert_child(
    node: &mut SurfaceNode,
    parent: &str,
    index: usize,
    new: SurfaceNode,
) -> Result<(), SurfaceError> {
    if node.id() == Some(parent) {
        return match node {
            SurfaceNode::Container { children, .. }
            | SurfaceNode::Row { children, .. }
            | SurfaceNode::Column { children, .. }
            | SurfaceNode::Section { children, .. } => {
                if index > children.len() {
                    return Err(SurfaceError::Budget(format!(
                        "index {index} is past the {} children of {parent:?}",
                        children.len()
                    )));
                }
                children.insert(index, new);
                Ok(())
            }
            _ => Err(SurfaceError::Invalid(format!(
                "node {parent:?} cannot hold children"
            ))),
        };
    }
    match node {
        SurfaceNode::Container { children, .. }
        | SurfaceNode::Row { children, .. }
        | SurfaceNode::Column { children, .. }
        | SurfaceNode::Section { children, .. } => {
            for child in children {
                if child.id() == Some(parent) {
                    return insert_child(child, parent, index, new);
                }
                if contains(child, parent) {
                    return insert_child(child, parent, index, new);
                }
            }
            Err(SurfaceError::UnknownNode(parent.to_string()))
        }
        SurfaceNode::Tabs { tabs, .. } => {
            for tab in tabs {
                if contains(&tab.content, parent) {
                    return insert_child(&mut tab.content, parent, index, new);
                }
            }
            Err(SurfaceError::UnknownNode(parent.to_string()))
        }
        SurfaceNode::Slot { fallback, .. } => fallback.as_mut().map_or(
            Err(SurfaceError::UnknownNode(parent.to_string())),
            |fallback| insert_child(fallback, parent, index, new),
        ),
        _ => Err(SurfaceError::UnknownNode(parent.to_string())),
    }
}

fn contains(node: &SurfaceNode, id: &str) -> bool {
    if node.id() == Some(id) {
        return true;
    }
    if let Some(children) = node.container_children() {
        if children.iter().any(|child| contains(child, id)) {
            return true;
        }
    }
    match node {
        SurfaceNode::Tabs { tabs, .. } => tabs.iter().any(|tab| contains(&tab.content, id)),
        SurfaceNode::Slot { fallback, .. } => {
            fallback.as_ref().is_some_and(|node| contains(node, id))
        }
        _ => false,
    }
}

fn remove_child(node: &mut SurfaceNode, id: &str) -> bool {
    match node {
        SurfaceNode::Container { children, .. }
        | SurfaceNode::Row { children, .. }
        | SurfaceNode::Column { children, .. }
        | SurfaceNode::Section { children, .. } => {
            if let Some(index) = children.iter().position(|child| child.id() == Some(id)) {
                children.remove(index);
                return true;
            }
            children.iter_mut().any(|child| remove_child(child, id))
        }
        SurfaceNode::Tabs { tabs, .. } => tabs
            .iter_mut()
            .any(|tab| remove_child(&mut tab.content, id)),
        SurfaceNode::Slot { fallback, .. } => fallback
            .as_mut()
            .is_some_and(|fallback| remove_child(fallback, id)),
        _ => false,
    }
}

fn set_text(node: &mut SurfaceNode, text: &str) -> Result<(), SurfaceError> {
    match node {
        SurfaceNode::Text { text: current, .. }
        | SurfaceNode::Heading { text: current, .. }
        | SurfaceNode::Markdown { text: current, .. }
        | SurfaceNode::Notification { text: current, .. } => {
            *current = text.to_string();
            Ok(())
        }
        SurfaceNode::Code { value, .. } => {
            *value = text.to_string();
            Ok(())
        }
        _ => Err(SurfaceError::Invalid(
            "set_text targets a text-like node only".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::document::{ActionMode, SurfaceDocument, SURFACE_SCHEMA_VERSION};
    use crate::surface::node::SurfaceNode;
    use crate::ui::Tone;

    fn document() -> SurfaceDocument {
        let mut bindings = BTreeMap::new();
        bindings.insert(
            "go".to_string(),
            ActionBinding {
                capability: "acme.demo/go".to_string(),
                operation: None,
                input: Value::Null,
                principal: crate::surface::document::DispatchPrincipal::Producer,
                required: true,
                mode: ActionMode::Once,
                debounce_ms: 0,
            },
        );
        SurfaceDocument {
            schema_version: SURFACE_SCHEMA_VERSION.to_string(),
            bindings,
            hidden_actions: Vec::new(),
            root: SurfaceNode::Column {
                id: Some("root".to_string()),
                children: vec![
                    SurfaceNode::Text {
                        id: Some("t".to_string()),
                        text: "old".to_string(),
                        tone: Tone::Neutral,
                    },
                    SurfaceNode::Button {
                        id: "b".to_string(),
                        label: "Go".to_string(),
                        action: "go".to_string(),
                        style: crate::ui::ButtonStyle::Primary,
                    },
                ],
            },
        }
    }

    #[test]
    fn set_text_and_insert_and_remove_apply() {
        let limits = SurfaceLimits::default();
        let next = apply_patch(
            &document(),
            &[
                PatchOp::SetText {
                    id: "t".to_string(),
                    text: "new".to_string(),
                },
                PatchOp::Insert {
                    parent: "root".to_string(),
                    index: 1,
                    node: SurfaceNode::Text {
                        id: Some("x".to_string()),
                        text: "inserted".to_string(),
                        tone: Tone::Neutral,
                    },
                },
            ],
            &limits,
        )
        .expect("patch");
        let text = next.node("t").expect("node");
        assert!(matches!(text, SurfaceNode::Text { text, .. } if text == "new"));
        assert!(next.node("x").is_some());
        let removed = apply_patch(
            &next,
            &[PatchOp::Remove {
                id: "x".to_string(),
            }],
            &limits,
        )
        .expect("remove");
        assert!(removed.node("x").is_none());
    }

    #[test]
    fn a_failed_patch_changes_nothing() {
        let limits = SurfaceLimits::default();
        let original = document();
        let error = apply_patch(
            &original,
            &[PatchOp::Replace {
                id: "t".to_string(),
                node: SurfaceNode::Text {
                    id: Some("t".to_string()),
                    text: "new".to_string(),
                    tone: Tone::Neutral,
                },
            }],
            &limits,
        )
        .expect("a same-id replace is legal");
        assert!(error.node("t").is_some());

        let error = apply_patch(
            &original,
            &[PatchOp::Remove {
                id: "missing".to_string(),
            }],
            &limits,
        )
        .expect_err("an unknown node is refused");
        assert!(matches!(error, SurfaceError::UnknownNode(_)));
        assert!(
            matches!(original.node("t"), Some(SurfaceNode::Text { text, .. }) if text == "old")
        );
    }

    #[test]
    fn a_duplicate_id_after_a_replace_is_refused_atomically() {
        let original = document();
        let result = apply_patch(
            &original,
            &[PatchOp::Insert {
                parent: "root".to_string(),
                index: 0,
                node: SurfaceNode::Text {
                    id: Some("t".to_string()),
                    text: "dupe".to_string(),
                    tone: Tone::Neutral,
                },
            }],
            &SurfaceLimits::default(),
        );
        assert!(result.is_err());
        assert!(
            matches!(original.node("t"), Some(SurfaceNode::Text { text, .. }) if text == "old")
        );
    }

    #[test]
    fn a_patch_over_the_op_ceiling_is_refused() {
        let limits = SurfaceLimits {
            max_patch_ops: 2,
            ..SurfaceLimits::default()
        };
        let ops: Vec<PatchOp> = (0..3)
            .map(|_| PatchOp::SetText {
                id: "t".to_string(),
                text: "x".to_string(),
            })
            .collect();
        assert!(matches!(
            apply_patch(&document(), &ops, &limits),
            Err(SurfaceError::Budget(_))
        ));
    }

    #[test]
    fn bindings_can_be_added_and_removed() {
        let limits = SurfaceLimits::default();
        let mut changed = BTreeMap::new();
        changed.insert(
            "more".to_string(),
            Some(ActionBinding {
                capability: "acme.demo/more".to_string(),
                operation: None,
                input: Value::Null,
                principal: crate::surface::document::DispatchPrincipal::Producer,
                required: true,
                mode: ActionMode::Once,
                debounce_ms: 0,
            }),
        );
        let next = apply_patch(&document(), &[PatchOp::SetBindings { changed }], &limits)
            .expect("add a binding");
        assert!(next.binding("more").is_some());
    }
}
