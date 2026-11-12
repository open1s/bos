//! The surface document: a schema 1.1 tree plus an action binding table.
//!
//! Validation is the host's normative gate. A provider validates too, but the
//! host validates again because a provider is not trusted. Every check here
//! runs before a document reaches a renderer, and a document that fails
//! changes nothing.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::{ValidationIssue, ValidationReport};
use crate::ui::UiDocument;

use super::contract::SurfaceContract;
use super::node::{Sizing, SurfaceNode};
use super::{SurfaceError, SurfaceId, SurfaceLimits};

/// The schema version this build writes.
pub const SURFACE_SCHEMA_VERSION: &str = "1.1";

/// The schema version this build still reads, for shipped panels.
pub const LEGACY_SCHEMA_VERSION: &str = "1.0";

/// One action a control may dispatch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionBinding {
    /// Provider-qualified capability id: provider slash name.
    pub capability: String,
    /// Optional named operation on that capability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    /// Input value, built from literals and the mapping forms.
    #[serde(default)]
    pub input: Value,
    /// Who performs the call.
    #[serde(default)]
    pub principal: DispatchPrincipal,
    /// Missing providers leave the control disabled when this is false.
    #[serde(default = "default_true")]
    pub required: bool,
    /// Whether a second press may start another call.
    #[serde(default)]
    pub mode: ActionMode,
    /// Debounce for change events, 0 through 500 milliseconds.
    #[serde(default)]
    pub debounce_ms: u16,
}

/// The principal that performs a bound call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DispatchPrincipal {
    /// The producer's own grants: a package, or the agent session.
    #[default]
    Producer,
    /// The reader's own authority, limited to a host allowlist.
    Reader,
}

/// How a binding treats a repeated press.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActionMode {
    /// Ignore a second press until the first settles.
    #[default]
    Once,
    /// Allow another call while one is in flight.
    Repeat,
}

/// One action a profile hides, with a reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HiddenAction {
    /// The binding key.
    pub action: String,
    /// Why this profile cannot draw it.
    pub reason: String,
}

/// A complete surface document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SurfaceDocument {
    /// Schema generation.
    pub schema_version: String,
    /// Binding key to bound capability.
    #[serde(default)]
    pub bindings: BTreeMap<String, ActionBinding>,
    /// Actions this profile hides, each with a reason.
    #[serde(default)]
    pub hidden_actions: Vec<HiddenAction>,
    /// Root node.
    pub root: SurfaceNode,
}

impl SurfaceDocument {
    /// Lower a schema 1.0 declarative view into a surface document.
    ///
    /// The result keeps schema 1.0, because its controls still name
    /// contribution ids rather than binding keys. A host that renders legacy
    /// panels keeps using the legacy action path; a host that wants the
    /// surface pipeline must migrate the package.
    pub fn from_ui(document: UiDocument) -> Self {
        Self {
            schema_version: document.schema_version,
            bindings: BTreeMap::new(),
            hidden_actions: Vec::new(),
            root: SurfaceNode::from(document.root),
        }
    }

    /// Validate against the default limits.
    pub fn validate(&self) -> ValidationReport {
        self.validate_with(&SurfaceLimits::default())
    }

    /// Validate against explicit limits.
    pub fn validate_with(&self, limits: &SurfaceLimits) -> ValidationReport {
        let mut report = ValidationReport::default();
        match self.schema_version.as_str() {
            SURFACE_SCHEMA_VERSION => {}
            LEGACY_SCHEMA_VERSION => {
                if !self.bindings.is_empty() {
                    report.error(
                        "surface.schema_version",
                        "a 1.0 document may not carry a binding table",
                    );
                }
                if !self.hidden_actions.is_empty() {
                    report.error(
                        "surface.schema_version",
                        "a 1.0 document may not carry hidden actions",
                    );
                }
            }
            other => report.error(
                "surface.schema_version",
                format!(
                    "unsupported surface schema {other:?}; this host reads {:?} and {:?}",
                    SURFACE_SCHEMA_VERSION, LEGACY_SCHEMA_VERSION
                ),
            ),
        }
        if self.bindings.len() > limits.max_bindings {
            report.error(
                "surface.bindings",
                format!(
                    "carries {} bindings; the ceiling is {}",
                    self.bindings.len(),
                    limits.max_bindings
                ),
            );
        }
        for (key, binding) in &self.bindings {
            validate_binding(key, binding, &mut report);
        }
        let mut seen = BTreeSet::new();
        for hidden in &self.hidden_actions {
            if hidden.reason.trim().is_empty() {
                report.error(
                    "surface.hidden_actions.reason",
                    format!("action {:?} hides without a reason", hidden.action),
                );
            }
            if !self.bindings.contains_key(&hidden.action) {
                report.error(
                    "surface.hidden_actions.action",
                    format!("action {:?} is not a binding", hidden.action),
                );
            }
            if !seen.insert(hidden.action.clone()) {
                report.error(
                    "surface.hidden_actions.action",
                    format!("action {:?} is hidden twice", hidden.action),
                );
            }
        }
        let mut walk = Walk {
            limits,
            report: &mut report,
            ids: BTreeSet::new(),
            nodes: 0,
            text: 0,
            slots: 0,
            legacy: self.schema_version == LEGACY_SCHEMA_VERSION,
            bindings: &self.bindings,
        };
        walk.node(&self.root, 1);
        match serde_json::to_vec(self) {
            Ok(bytes) if bytes.len() > limits.max_bytes => report.error(
                "surface",
                format!(
                    "document is {} bytes; the ceiling is {}",
                    bytes.len(),
                    limits.max_bytes
                ),
            ),
            Err(error) => {
                report.error("surface", format!("document cannot be serialized: {error}"))
            }
            Ok(_) => {}
        }
        report
    }

    /// Validate a document against the contract that offered it.
    ///
    /// This is the mount-time check: every binding must name an action the
    /// contract declares, and its capability must be the one that action
    /// declares. A document cannot reach a capability the contract did not
    /// advertise, even if the producer can call it elsewhere.
    pub fn validate_against(
        &self,
        contract: &SurfaceContract,
        limits: &SurfaceLimits,
    ) -> ValidationReport {
        let mut report = self.validate_with(limits);
        if self.schema_version == LEGACY_SCHEMA_VERSION {
            return report;
        }
        for (key, binding) in &self.bindings {
            match contract.action(key) {
                None => report.error(
                    "surface.bindings",
                    format!("binding {key:?} is not declared by the contract"),
                ),
                Some(spec) if spec.capability != binding.capability => report.error(
                    "surface.bindings.capability",
                    format!(
                        "binding {key:?} names {:?}; the contract declares {:?}",
                        binding.capability, spec.capability
                    ),
                ),
                Some(_) => {}
            }
        }
        report
    }

    /// Lower this document to the schema 1.0 view the existing renderer reads.
    ///
    /// Action bindings are not carried: a 1.0 control names a contribution id
    /// directly, so a 1.1 document rendered this way keeps its binding keys in
    /// the control but loses the table. This path is for the legacy panel
    /// renderer, not for the surface pipeline.
    pub fn to_ui_document(&self) -> UiDocument {
        UiDocument {
            schema_version: LEGACY_SCHEMA_VERSION.to_string(),
            root: self.root.to_ui_node(),
        }
    }

    /// Validate a document that arrived from another node.
    ///
    /// A remote frame may bind only capabilities hosted on the node that
    /// produced it. A local capability id in a remote frame is refused, because
    /// otherwise a peer could use the reader's click as local authority.
    pub fn validate_remote(&self, remote_provider: &str) -> ValidationReport {
        let mut report = self.validate();
        for (key, binding) in &self.bindings {
            let provider = binding
                .capability
                .split_once('/')
                .map(|(provider, _)| provider)
                .unwrap_or_default();
            if provider != remote_provider {
                report.error(
                    "surface.bindings.remote",
                    format!(
                        "binding {key:?} names {:?}, which is not hosted by the remote node {remote_provider:?}",
                        binding.capability
                    ),
                );
            }
        }
        report
    }

    /// Look up one binding.
    pub fn binding(&self, key: &str) -> Option<&ActionBinding> {
        self.bindings.get(key)
    }

    /// Iterate every node in preorder.
    pub fn nodes(&self) -> Vec<&SurfaceNode> {
        let mut out = Vec::new();
        collect(&self.root, &mut out);
        out
    }

    /// Find one node by id.
    pub fn node(&self, id: &str) -> Option<&SurfaceNode> {
        self.nodes().into_iter().find(|node| node.id() == Some(id))
    }
}

fn collect<'a>(node: &'a SurfaceNode, out: &mut Vec<&'a SurfaceNode>) {
    out.push(node);
    if let Some(children) = node.container_children() {
        for child in children {
            collect(child, out);
        }
    }
    if let SurfaceNode::Tabs { tabs, .. } = node {
        for tab in tabs {
            collect(&tab.content, out);
        }
    }
    if let SurfaceNode::Slot {
        fallback: Some(fallback),
        ..
    } = node
    {
        collect(fallback, out);
    }
}

fn validate_binding(key: &str, binding: &ActionBinding, report: &mut ValidationReport) {
    if key.trim().is_empty() {
        report.error("surface.bindings", "binding keys must not be empty");
    }
    if SurfaceId::parse(binding.capability.clone()).is_err() {
        report.error(
            "surface.bindings.capability",
            format!(
                "binding {key:?} names {:?}, which is not a provider-qualified capability",
                binding.capability
            ),
        );
    }
    if binding.debounce_ms > 500 {
        report.error(
            "surface.bindings.debounce_ms",
            format!(
                "binding {key:?} debounces {}ms; the ceiling is 500ms",
                binding.debounce_ms
            ),
        );
    }
    if let Some(operation) = &binding.operation {
        if operation.trim().is_empty() || operation.contains(char::is_whitespace) {
            report.error(
                "surface.bindings.operation",
                format!("binding {key:?} names an empty or spaced operation"),
            );
        }
    }
}

struct Walk<'a> {
    limits: &'a SurfaceLimits,
    report: &'a mut ValidationReport,
    ids: BTreeSet<String>,
    nodes: usize,
    text: usize,
    slots: usize,
    legacy: bool,
    bindings: &'a BTreeMap<String, ActionBinding>,
}

impl Walk<'_> {
    fn node(&mut self, node: &SurfaceNode, depth: usize) {
        self.nodes += 1;
        if self.nodes > self.limits.max_nodes {
            if self.nodes == self.limits.max_nodes + 1 {
                self.report.error(
                    "surface.nodes",
                    format!("document has more than {} nodes", self.limits.max_nodes),
                );
            }
            return;
        }
        if depth > self.limits.max_depth {
            self.report.error(
                "surface.depth",
                format!(
                    "document nests deeper than {} levels",
                    self.limits.max_depth
                ),
            );
            return;
        }
        if node.is_interactive() && node.id().is_none_or(|id| id.trim().is_empty()) {
            self.report.error(
                "surface.id",
                "every interactive node must carry a non-empty id",
            );
        }
        if let Some(id) = node.id() {
            if id.trim().is_empty() {
                self.report
                    .error("surface.id", "a node id must not be empty");
            } else if !self.ids.insert(id.to_string()) {
                self.report
                    .error("surface.id", format!("node id {id:?} is used twice"));
            }
        }
        self.leaves(node);
        let mut slots_here = 0usize;
        if let Some(children) = node.container_children() {
            for child in children {
                if child.is_slot() {
                    slots_here += 1;
                }
                self.node(child, depth + 1);
            }
            if slots_here > self.limits.max_slot_fanout {
                self.report.error(
                    "surface.slot.fanout",
                    format!(
                        "a node holds {slots_here} slots; the ceiling is {}",
                        self.limits.max_slot_fanout
                    ),
                );
            }
        }
        if let SurfaceNode::Tabs { tabs, .. } = node {
            for tab in tabs {
                if tab.id.trim().is_empty() {
                    self.report
                        .error("surface.tabs.id", "tab ids must not be empty");
                }
                self.node(&tab.content, depth + 1);
            }
        }
        if let SurfaceNode::Slot {
            surface,
            version,
            required,
            props,
            sizing,
            fallback,
            ..
        } = node
        {
            self.slots += 1;
            if SurfaceId::parse(surface.clone()).is_err() {
                self.report.error(
                    "surface.slot.surface",
                    format!("slot names {surface:?}, which is not a provider-qualified surface id"),
                );
            }
            if let Some(version) = version {
                if semver::VersionReq::parse(version).is_err() {
                    self.report.error(
                        "surface.slot.version",
                        format!("slot version range {version:?} is not valid semver"),
                    );
                }
            }
            if !props.is_null() && !props.is_object() {
                self.report.error(
                    "surface.slot.props",
                    "slot props must be an object or absent",
                );
            }
            self.sizing(sizing);
            match (required, fallback) {
                (true, Some(_)) => self.report.warn(
                    "surface.slot.fallback",
                    "a required slot declares a fallback, which the host will not draw",
                ),
                (false, None) => self.report.error(
                    "surface.slot.fallback",
                    "an optional slot must declare a fallback",
                ),
                _ => {}
            }
            if let Some(fallback) = fallback {
                self.node(fallback, depth + 1);
            }
        }
        if let SurfaceNode::List { items, .. } = node {
            for item in items {
                if let Some(action) = &item.action {
                    self.action("surface.list.item.action", action);
                }
            }
        }
    }

    fn sizing(&mut self, sizing: &Sizing) {
        if !(0.1..=20.0).contains(&sizing.weight) {
            self.report.error(
                "surface.slot.sizing.weight",
                "weight must be between 0.1 and 20",
            );
        }
        if sizing.max > 0 && sizing.min > sizing.max {
            self.report
                .error("surface.slot.sizing", "min must not exceed a non-zero max");
        }
    }

    fn leaves(&mut self, node: &SurfaceNode) {
        match node {
            SurfaceNode::Text { text, .. }
            | SurfaceNode::Heading { text, .. }
            | SurfaceNode::Markdown { text, .. }
            | SurfaceNode::Notification { text, .. } => self.text_leaf("surface.text", text),
            SurfaceNode::Button {
                id, label, action, ..
            } => {
                self.text_leaf("surface.button.label", label);
                self.control(id, action);
            }
            SurfaceNode::Input {
                id,
                label,
                value,
                placeholder,
                action,
            } => {
                self.text_leaf("surface.input.label", label);
                self.text_leaf("surface.input.value", value);
                self.text_leaf("surface.input.placeholder", placeholder);
                self.control(id, action);
            }
            SurfaceNode::Select {
                id,
                label,
                value,
                options,
                action,
            } => {
                self.text_leaf("surface.select.label", label);
                self.text_leaf("surface.select.value", value);
                if options.len() > 200 {
                    self.report
                        .error("surface.select.options", "must contain at most 200 options");
                }
                for option in options {
                    self.text_leaf("surface.select.option.label", &option.label);
                    self.text_leaf("surface.select.option.value", &option.value);
                }
                self.control(id, action);
            }
            SurfaceNode::Checkbox {
                id, label, action, ..
            } => {
                self.text_leaf("surface.checkbox.label", label);
                self.control(id, action);
            }
            SurfaceNode::List { items, .. } => {
                for item in items {
                    self.text_leaf("surface.list.item.title", &item.title);
                    self.text_leaf("surface.list.item.detail", &item.detail);
                }
            }
            SurfaceNode::Table {
                columns,
                rows,
                widths,
                ..
            } => {
                if !widths.is_empty()
                    && (widths.len() != columns.len()
                        || widths.iter().any(|w| !(*w > 0.0 && *w <= 20.0)))
                {
                    self.report.error(
                        "surface.table.widths",
                        "must be empty or carry one weight in (0, 20] per column",
                    );
                }
                for column in columns {
                    self.text_leaf("surface.table.column", column);
                }
                for row in rows {
                    if row.len() != columns.len() {
                        self.report.error(
                            "surface.table.row",
                            "every row must have exactly one cell per column",
                        );
                    }
                    for cell in row {
                        self.text_leaf("surface.table.cell", cell);
                    }
                }
            }
            SurfaceNode::Section { title, .. } => self.text_leaf("surface.section.title", title),
            SurfaceNode::Tree { nodes, .. } => {
                for node in nodes {
                    self.tree(node);
                }
            }
            SurfaceNode::Tabs { tabs, .. } => {
                for tab in tabs {
                    self.text_leaf("surface.tabs.label", &tab.label);
                }
            }
            SurfaceNode::Progress { value, label, .. } => {
                if !(0.0..=1.0).contains(value) {
                    self.report
                        .error("surface.progress.value", "must be between 0.0 and 1.0");
                }
                self.text_leaf("surface.progress.label", label);
            }
            SurfaceNode::Code {
                language, value, ..
            } => {
                self.text_leaf("surface.code.language", language);
                self.text_leaf("surface.code.value", value);
            }
            SurfaceNode::Image { src, alt, .. } => {
                if !valid_image_source(src) {
                    self.report.error(
                        "surface.image.src",
                        "must be an asset/ package-relative path; remote images are refused",
                    );
                }
                self.text_leaf("surface.image.alt", alt);
            }
            SurfaceNode::Spacer { size, .. } => self.spacer(*size),
            _ => {}
        }
    }

    fn spacer(&mut self, size: u16) {
        if size > 512 {
            self.report
                .error("surface.spacer.size", "must be at most 512");
        }
    }

    fn control(&mut self, id: &str, action: &str) {
        if id.trim().is_empty() {
            self.report
                .error("surface.id", "an interactive control needs an id");
        }
        self.action("surface.action", action);
    }

    fn action(&mut self, field: &str, action: &str) {
        if action.trim().is_empty() {
            self.report.error(field, "a control must name an action");
            return;
        }
        if self.bindings.contains_key(action) {
            return;
        }
        if self.legacy && is_contribution_id(action) {
            return;
        }
        self.report.error(
            field,
            format!("{action:?} is not a binding this document declares"),
        );
    }

    fn tree(&mut self, node: &crate::ui::UiTreeNode) {
        self.nodes += 1;
        if self.nodes > self.limits.max_nodes {
            return;
        }
        self.text_leaf("surface.tree.label", &node.label);
        for child in &node.children {
            self.tree(child);
        }
    }

    fn text_leaf(&mut self, field: &str, text: &str) {
        let characters = text.chars().count();
        if characters > self.limits.max_text {
            self.report.error(
                field,
                format!("must be at most {} characters", self.limits.max_text),
            );
        }
        self.text += characters;
        if self.text > self.limits.max_total_text {
            self.report.error(
                "surface.text",
                format!(
                    "document carries more than {} characters of text",
                    self.limits.max_total_text
                ),
            );
        }
    }
}

fn default_true() -> bool {
    true
}

fn is_contribution_id(value: &str) -> bool {
    value.split('.').count() >= 2
        && value.split('.').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
}

fn valid_image_source(src: &str) -> bool {
    let Some(rest) = src.strip_prefix("asset/") else {
        return false;
    };
    !rest.is_empty()
        && !rest.starts_with('/')
        && !rest.split('/').any(|part| part == ".." || part.is_empty())
}

/// Ensure a report is valid, or turn its errors into one message.
pub fn ensure_valid(report: ValidationReport) -> Result<(), SurfaceError> {
    if report.is_valid() {
        return Ok(());
    }
    Err(SurfaceError::Invalid(errors(&report)))
}

fn errors(report: &ValidationReport) -> String {
    report
        .errors
        .iter()
        .map(ValidationIssue::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::Tone;

    fn document(root: SurfaceNode) -> SurfaceDocument {
        SurfaceDocument {
            schema_version: SURFACE_SCHEMA_VERSION.to_string(),
            bindings: BTreeMap::new(),
            hidden_actions: Vec::new(),
            root,
        }
    }

    fn button(action: &str) -> SurfaceNode {
        SurfaceNode::Button {
            id: "refresh".to_string(),
            label: "Refresh".to_string(),
            action: action.to_string(),
            style: crate::ui::ButtonStyle::Primary,
        }
    }

    #[test]
    fn a_valid_document_passes() {
        let mut document = document(SurfaceNode::Column {
            id: Some("root".to_string()),
            children: vec![button("refresh")],
        });
        document.bindings.insert(
            "refresh".to_string(),
            ActionBinding {
                capability: "acme.git/refresh".to_string(),
                operation: None,
                input: Value::Null,
                principal: DispatchPrincipal::Producer,
                required: true,
                mode: ActionMode::Once,
                debounce_ms: 0,
            },
        );
        ensure_valid(document.validate()).expect("valid");
    }

    #[test]
    fn an_interactive_node_needs_an_id() {
        let document = document(SurfaceNode::Button {
            id: String::new(),
            label: "Go".to_string(),
            action: "go".to_string(),
            style: crate::ui::ButtonStyle::Primary,
        });
        let report = document.validate();
        assert!(!report.is_valid());
        assert!(report
            .errors
            .iter()
            .any(|issue| issue.field == "surface.id"));
    }

    #[test]
    fn an_unknown_binding_is_refused() {
        let document = document(SurfaceNode::Column {
            id: None,
            children: vec![button("missing")],
        });
        let report = document.validate();
        assert!(!report.is_valid());
        assert!(report
            .errors
            .iter()
            .any(|issue| issue.field == "surface.action"));
    }

    #[test]
    fn duplicate_ids_are_refused() {
        let document = document(SurfaceNode::Column {
            id: None,
            children: vec![
                SurfaceNode::Text {
                    id: Some("same".to_string()),
                    text: "a".to_string(),
                    tone: Tone::Neutral,
                },
                SurfaceNode::Text {
                    id: Some("same".to_string()),
                    text: "b".to_string(),
                    tone: Tone::Neutral,
                },
            ],
        });
        assert!(!document.validate().is_valid());
    }

    #[test]
    fn lowering_to_1_0_drops_ids_and_slots() {
        let document = SurfaceDocument {
            schema_version: SURFACE_SCHEMA_VERSION.to_string(),
            bindings: BTreeMap::new(),
            hidden_actions: Vec::new(),
            root: SurfaceNode::Column {
                id: Some("root".to_string()),
                children: vec![
                    SurfaceNode::Text {
                        id: Some("t".to_string()),
                        text: "hello".to_string(),
                        tone: Tone::Neutral,
                    },
                    SurfaceNode::Slot {
                        id: None,
                        surface: "acme.git/status".to_string(),
                        version: None,
                        required: false,
                        props: serde_json::Value::Null,
                        sizing: Sizing::default(),
                        fallback: Some(Box::new(SurfaceNode::Text {
                            id: None,
                            text: "fallback".to_string(),
                            tone: Tone::Muted,
                        })),
                    },
                ],
            },
        };
        let lowered = document.to_ui_document();
        assert_eq!(lowered.schema_version, LEGACY_SCHEMA_VERSION);
        match lowered.root {
            crate::ui::UiNode::Column { children } => {
                assert_eq!(children.len(), 2);
                assert!(
                    matches!(&children[0], crate::ui::UiNode::Text { text, .. } if text == "hello")
                );
                assert!(
                    matches!(&children[1], crate::ui::UiNode::Text { text, .. } if text == "fallback")
                );
            }
            other => panic!("expected a column, got {other:?}"),
        }
    }

    #[test]
    fn a_remote_frame_may_bind_only_its_own_node() {
        let mut document = document(SurfaceNode::Column {
            id: None,
            children: vec![button("refresh")],
        });
        document.bindings.insert(
            "refresh".to_string(),
            ActionBinding {
                capability: "acme.git/refresh".to_string(),
                operation: None,
                input: Value::Null,
                principal: DispatchPrincipal::Producer,
                required: true,
                mode: ActionMode::Once,
                debounce_ms: 0,
            },
        );
        ensure_valid(document.validate_remote("acme.git")).expect("the producing node may bind it");
        let report = document.validate_remote("other.node");
        assert!(!report.is_valid());
        assert!(report
            .errors
            .iter()
            .any(|issue| issue.field == "surface.bindings.remote"));
    }

    #[test]
    fn a_legacy_1_0_document_uses_contribution_ids() {
        let legacy: UiDocument = serde_json::from_value(serde_json::json!({
            "schema_version": "1.0",
            "root": {
                "type": "column",
                "children": [
                    { "type": "button", "label": "Refresh", "action": "git.refresh" }
                ]
            }
        }))
        .expect("document");
        let surface = SurfaceDocument::from_ui(legacy);
        ensure_valid(surface.validate()).expect("a 1.0 document still validates");
        assert_eq!(surface.schema_version, LEGACY_SCHEMA_VERSION);
    }

    #[test]
    fn a_1_1_document_may_not_use_a_bare_contribution_id() {
        let mut document = document(SurfaceNode::Column {
            id: None,
            children: vec![button("git.refresh")],
        });
        document.schema_version = SURFACE_SCHEMA_VERSION.to_string();
        assert!(!document.validate().is_valid());
    }

    #[test]
    fn an_optional_slot_needs_a_fallback() {
        let document = document(SurfaceNode::Slot {
            id: None,
            surface: "acme.git/status".to_string(),
            version: Some("^1".to_string()),
            required: false,
            props: Value::Null,
            sizing: Sizing::default(),
            fallback: None,
        });
        let report = document.validate();
        assert!(!report.is_valid());
        assert!(report
            .errors
            .iter()
            .any(|issue| issue.field == "surface.slot.fallback"));
    }

    #[test]
    fn a_bad_version_range_is_refused() {
        let document = document(SurfaceNode::Slot {
            id: None,
            surface: "acme.git/status".to_string(),
            version: Some("one".to_string()),
            required: true,
            props: Value::Null,
            sizing: Sizing::default(),
            fallback: None,
        });
        assert!(!document.validate().is_valid());
    }

    #[test]
    fn remote_images_are_refused() {
        for src in ["https://example.com/a.png", "asset/../escape.png"] {
            let document = document(SurfaceNode::Image {
                id: None,
                src: src.to_string(),
                alt: String::new(),
            });
            assert!(!document.validate().is_valid(), "{src}");
        }
    }
}
