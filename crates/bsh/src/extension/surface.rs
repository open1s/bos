//! The BSH host side of the dynamic UI surface framework.
//!
//! This is the first host adapter over `ferris_sdk::surface`: it owns one
//! surface instance per mounted panel, validates the shipped declarative view
//! through the schema 1.1 surface document, and keeps the host slot catalogue
//! that placement rules refer to.
//!
//! The GPUI renderer still reads the panel's own live document for now; this
//! adapter is the admission and ownership layer the renderer will read from
//! next. Keeping it live means every panel is validated twice with the same
//! limits the framework enforces, and a rebuild disposes exactly the instances
//! it unmounted.

use std::collections::BTreeMap;

use ferris_sdk::surface::{
    InstanceId, IsolationClass, Profile, Revision, StateModel, SurfaceContract, SurfaceDocument,
    SurfaceEngine, SurfaceFrame, SurfaceId, SurfaceLimits, SurfaceNode,
};
use ferris_sdk::Version;

/// Where the host is willing to mount a surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum HostSlot {
    /// The primary work surface.
    Main,
    /// The left rail.
    Left,
    /// The right rail.
    Right,
    /// The bottom dock.
    Bottom,
    /// A settings page body.
    SettingsPage,
    /// One command-palette row.
    PaletteRow,
    /// A turn result inside the transcript.
    TranscriptInline,
    /// First-run and blank-session content.
    Hero,
}

impl HostSlot {
    /// The stable slot id.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Main => "workspace.main",
            Self::Left => "workspace.left",
            Self::Right => "workspace.right",
            Self::Bottom => "workspace.bottom",
            Self::SettingsPage => "settings.page",
            Self::PaletteRow => "command.palette.row",
            Self::TranscriptInline => "transcript.inline",
            Self::Hero => "empty.hero",
        }
    }

    /// How many presenters the slot admits.
    pub(crate) fn occupancy(self) -> Occupancy {
        match self {
            Self::Main | Self::SettingsPage => Occupancy::Single,
            Self::PaletteRow | Self::TranscriptInline => Occupancy::Inline,
            Self::Left | Self::Right | Self::Bottom | Self::Hero => Occupancy::Stacked,
        }
    }

    /// Whether an agent-authored surface may mount here.
    pub(crate) fn accepts_agents(self) -> bool {
        matches!(self, Self::Main | Self::TranscriptInline | Self::Hero)
    }

    /// Whether only first-party providers may fill the slot.
    pub(crate) fn first_party_only(self) -> bool {
        matches!(self, Self::SettingsPage | Self::PaletteRow)
    }

    /// The panel location this slot corresponds to.
    pub(crate) fn from_panel_location(location: ferris_sdk::ui::PanelLocation) -> Self {
        match location {
            ferris_sdk::ui::PanelLocation::Left => Self::Left,
            ferris_sdk::ui::PanelLocation::Right => Self::Right,
            ferris_sdk::ui::PanelLocation::Bottom => Self::Bottom,
            ferris_sdk::ui::PanelLocation::Center => Self::Main,
        }
    }
}

/// How many surfaces a slot admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Occupancy {
    /// One presenter; a second mount is a conflict.
    Single,
    /// Ordered and host-tabbed.
    Stacked,
    /// Many small rows.
    Inline,
}

/// One panel offered to the surface host.
pub(crate) struct PanelAdmission<'a> {
    /// The qualified panel id.
    pub(crate) panel_id: &'a str,
    /// Where the panel asked to be mounted.
    pub(crate) location: ferris_sdk::ui::PanelLocation,
    /// The declared view, when the panel shipped one.
    pub(crate) document: Option<&'a ferris_sdk::ui::UiDocument>,
}

/// What one synchronisation did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SyncReport {
    /// Panels newly admitted.
    pub(crate) mounted: usize,
    /// Instances disposed because their panel is gone.
    pub(crate) disposed: usize,
    /// Panels whose document was refused, with the reason.
    pub(crate) errors: Vec<String>,
}

/// One inspector row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SurfaceRow {
    /// The qualified panel id.
    pub(crate) panel_id: String,
    /// The host slot id.
    pub(crate) slot: &'static str,
    /// The provider generation.
    pub(crate) generation: u64,
    /// The current revision.
    pub(crate) revision: u64,
    /// The lifecycle state.
    pub(crate) state: ferris_sdk::surface::InstanceState,
    /// Why the panel was refused, when it was.
    pub(crate) refused: Option<String>,
}

struct MountedPanel {
    instance: InstanceId,
    slot: HostSlot,
    contract: SurfaceContract,
}

/// The host's surface instances, one per mounted panel.
#[derive(Default)]
pub(crate) struct SurfaceHost {
    engine: SurfaceEngine,
    limits: SurfaceLimits,
    mounted: BTreeMap<String, MountedPanel>,
    refused: BTreeMap<String, String>,
}

impl SurfaceHost {
    /// A host with the default surface limits.
    pub(crate) fn new() -> Self {
        Self {
            engine: SurfaceEngine::new(),
            limits: SurfaceLimits::default(),
            mounted: BTreeMap::new(),
            refused: BTreeMap::new(),
        }
    }

    /// Admit new panels and dispose the instances whose panel is gone.
    ///
    /// A panel whose declared view fails validation is recorded and skipped;
    /// one bad package must not take the panel list down.
    pub(crate) fn sync(&mut self, admissions: &[PanelAdmission<'_>]) -> SyncReport {
        let mut report = SyncReport::default();
        let desired: BTreeMap<&str, &PanelAdmission<'_>> = admissions
            .iter()
            .map(|admission| (admission.panel_id, admission))
            .collect();
        let stale: Vec<String> = self
            .mounted
            .keys()
            .filter(|panel_id| !desired.contains_key(panel_id.as_str()))
            .cloned()
            .collect();
        for panel_id in stale {
            if let Some(panel) = self.mounted.remove(&panel_id) {
                let _ = self.engine.dispose(panel.instance);
                report.disposed += 1;
            }
        }
        for (panel_id, admission) in desired {
            if self.mounted.contains_key(panel_id) {
                continue;
            }
            match self.admit(panel_id, admission) {
                Ok(instance) => {
                    self.mounted.insert(
                        panel_id.to_string(),
                        MountedPanel {
                            instance,
                            slot: HostSlot::from_panel_location(admission.location),
                            contract: self
                                .engine
                                .instance(instance)
                                .expect("just admitted")
                                .contract()
                                .clone(),
                        },
                    );
                    self.refused.remove(panel_id);
                    report.mounted += 1;
                }
                Err(error) => {
                    self.refused.insert(panel_id.to_string(), error.clone());
                    report.errors.push(format!("{panel_id}: {error}"));
                }
            }
        }
        report
    }

    /// Replace a panel's provider generation.
    ///
    /// A rebind is a provider replacement, so the instance gets a new
    /// generation and a fresh revision floor: an old card or action that
    /// referenced the retired generation is refused rather than applied to a
    /// provider it never observed.
    pub(crate) fn renew(&mut self, panel_id: &str) -> Result<(), String> {
        let Some(mounted) = self.mounted.get(panel_id) else {
            return Ok(());
        };
        let instance = mounted.instance;
        let contract = mounted.contract.clone();
        self.engine
            .replace(instance, contract, empty_document(), &self.limits)
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// The generation a mounted panel is at, for diagnostics and tests.
    pub(crate) fn generation_of(&self, panel_id: &str) -> Option<u64> {
        let mounted = self.mounted.get(panel_id)?;
        let instance = self.engine.instance(mounted.instance).ok()?;
        Some(instance.generation().value())
    }

    /// One row per mounted panel, for an inspector.
    pub(crate) fn rows(&self) -> Vec<SurfaceRow> {
        self.mounted
            .iter()
            .map(|(panel_id, panel)| {
                let (generation, revision, state) = self
                    .engine
                    .instance(panel.instance)
                    .map(|instance| {
                        (
                            instance.generation().value(),
                            instance.revision().value(),
                            instance.state(),
                        )
                    })
                    .unwrap_or((0, 0, ferris_sdk::surface::InstanceState::Disposed));
                SurfaceRow {
                    panel_id: panel_id.clone(),
                    slot: panel.slot.id(),
                    generation,
                    revision,
                    state,
                    refused: self.refused.get(panel_id).cloned(),
                }
            })
            .collect()
    }

    /// Apply a frame that arrived from a transport.
    ///
    /// The frame must name a live instance and generation; anything older is
    /// refused by the engine before it can move a revision.
    pub(crate) fn apply_frame(&mut self, frame: &SurfaceFrame) -> Result<Revision, String> {
        self.engine
            .apply_frame(frame, &self.limits)
            .map_err(|error| error.to_string())
    }

    /// Dispose the instance a transport asked to close.
    pub(crate) fn close_instance(&mut self, instance: InstanceId) -> bool {
        let Some(panel_id) = self
            .mounted
            .iter()
            .find(|(_, panel)| panel.instance == instance)
            .map(|(panel_id, _)| panel_id.clone())
        else {
            return false;
        };
        if let Some(panel) = self.mounted.remove(&panel_id) {
            let _ = self.engine.dispose(panel.instance);
            return true;
        }
        false
    }

    /// How many panels were refused and are not mounted.
    pub(crate) fn refused_count(&self) -> usize {
        self.refused.len()
    }

    /// How many instances the host owns.
    pub(crate) fn len(&self) -> usize {
        self.mounted.len()
    }

    /// The host slot a mounted panel occupies.
    pub(crate) fn slot_of(&self, panel_id: &str) -> Option<HostSlot> {
        self.mounted.get(panel_id).map(|panel| panel.slot)
    }

    /// The instance behind a mounted panel.
    pub(crate) fn instance_of(&self, panel_id: &str) -> Option<InstanceId> {
        self.mounted.get(panel_id).map(|panel| panel.instance)
    }

    /// Why a panel was refused, when it was.
    pub(crate) fn refusal(&self, panel_id: &str) -> Option<&str> {
        self.refused.get(panel_id).map(String::as_str)
    }

    /// The document and revision the renderer should draw.
    ///
    /// Slots are resolved against the other mounted panels before the
    /// document is returned, so a panel that composes another panel draws its
    /// content. A missing or cyclic child keeps the declared fallback. The
    /// child's own action bindings are not merged into the parent, so an
    /// expanded child is drawn read-only; mounting it as its own view keeps
    /// its actions.
    pub(crate) fn view(&self, panel_id: &str) -> Option<(SurfaceDocument, Revision)> {
        let mounted = self.mounted.get(panel_id)?;
        let instance = self.engine.instance(mounted.instance).ok()?;
        let document = instance.document()?.clone();
        let mut chain = vec![panel_id.to_string()];
        let expanded = self.expand_document(&document, 0, &mut chain);
        // Composition is bounded too: an expanded tree that exceeds the same
        // node and byte ceilings a frame obeys is not drawn, and the panel
        // keeps its own document instead of a region that would blow the
        // frame budget. Action grammar is not re-validated here: a prefixed
        // child action is a routing key, not a 1.0 contribution id.
        let fits = expanded.nodes().len() <= self.limits.max_nodes
            && serde_json::to_vec(&expanded)
                .map(|bytes| bytes.len() <= self.limits.max_bytes)
                .unwrap_or(false);
        let expanded = if fits { expanded } else { document };
        Some((expanded, instance.revision()))
    }

    fn expand_document(
        &self,
        document: &SurfaceDocument,
        depth: usize,
        chain: &mut Vec<String>,
    ) -> SurfaceDocument {
        let Ok(value) = serde_json::to_value(&document.root) else {
            return document.clone();
        };
        let expanded = self.expand_value(value, depth, chain);
        let root = serde_json::from_value(expanded).unwrap_or_else(|_| document.root.clone());
        SurfaceDocument {
            schema_version: document.schema_version.clone(),
            bindings: document.bindings.clone(),
            hidden_actions: document.hidden_actions.clone(),
            root,
        }
    }

    fn expand_value(
        &self,
        value: serde_json::Value,
        depth: usize,
        chain: &mut Vec<String>,
    ) -> serde_json::Value {
        match value {
            serde_json::Value::Object(mut map) => {
                if map.get("type").and_then(serde_json::Value::as_str) == Some("slot") {
                    if let Some(child) = self.slot_target(&map) {
                        if depth < self.limits.max_slot_depth && !chain.contains(&child.0) {
                            if let Ok(instance) = self.engine.instance(child.1) {
                                if let Some(document) = instance.document() {
                                    if let Ok(root) = serde_json::to_value(&document.root) {
                                        chain.push(child.0.clone());
                                        let mut expanded =
                                            self.expand_value(root, depth + 1, chain);
                                        chain.pop();
                                        // A child control's action names its own panel,
                                        // so the parent view can route the press to the
                                        // provider that owns the region.
                                        prefix_actions(
                                            &mut expanded,
                                            &format!("{}\u{1f}", child.0),
                                        );
                                        return expanded;
                                    }
                                }
                            }
                        }
                    }
                    if let Some(fallback) = map.remove("fallback") {
                        return self.expand_value(fallback, depth, chain);
                    }
                    return serde_json::json!({"type": "container", "children": []});
                }
                for child in map.values_mut() {
                    let taken = std::mem::take(child);
                    *child = self.expand_value(taken, depth, chain);
                }
                serde_json::Value::Object(map)
            }
            serde_json::Value::Array(items) => serde_json::Value::Array(
                items
                    .into_iter()
                    .map(|item| self.expand_value(item, depth, chain))
                    .collect(),
            ),
            other => other,
        }
    }

    fn slot_target(
        &self,
        map: &serde_json::Map<String, serde_json::Value>,
    ) -> Option<(String, InstanceId)> {
        let surface = map.get("surface")?.as_str()?;
        let wanted = SurfaceId::parse(surface).ok()?;
        self.mounted
            .iter()
            .find(|(_, panel)| panel.contract.surface == wanted)
            .map(|(panel_id, panel)| (panel_id.clone(), panel.instance))
    }

    /// Apply a live panel card as the next frame of the mounted instance.
    pub(crate) fn apply(
        &mut self,
        panel_id: &str,
        view: &ferris_sdk::ui::UiDocument,
    ) -> Result<(SurfaceDocument, Revision), String> {
        let instance = self
            .mounted
            .get(panel_id)
            .map(|panel| panel.instance)
            .ok_or_else(|| format!("panel {panel_id} is not mounted"))?;
        let current = self
            .engine
            .instance(instance)
            .map_err(|error| error.to_string())?;
        let generation = current.generation();
        let revision = current.revision().next();
        let document = SurfaceDocument::from_ui(view.clone());
        let frame = SurfaceFrame::full(instance, generation, revision, document);
        self.engine
            .apply_frame(&frame, &self.limits)
            .map_err(|error| error.to_string())?;
        let applied = self
            .engine
            .instance(instance)
            .map_err(|error| error.to_string())?
            .document()
            .cloned()
            .ok_or_else(|| "the applied frame has no document".to_string())?;
        Ok((applied, revision))
    }

    /// Refuse an action stamped with a revision the surface has moved past.
    pub(crate) fn accept(&self, panel_id: &str, revision: u64) -> Result<(), String> {
        let mounted = self
            .mounted
            .get(panel_id)
            .ok_or_else(|| format!("panel {panel_id} is not mounted"))?;
        let instance = self
            .engine
            .instance(mounted.instance)
            .map_err(|error| error.to_string())?;
        if !instance.state().accepts_actions() {
            return Err(format!("the panel surface is {}", instance.state()));
        }
        if instance.revision().value() != revision {
            return Err(format!(
                "stale action at revision {revision}; the surface is at {}",
                instance.revision().value()
            ));
        }
        Ok(())
    }

    /// The engine, for a diagnostics pane.
    pub(crate) fn engine(&self) -> &SurfaceEngine {
        &self.engine
    }

    /// Mount a surface document that did not come from a panel file.
    ///
    /// This is the path an agent turn or another runtime will use: the
    /// document is already a schema 1.1 surface tree, and the host admits it
    /// under the same contract, limits and disposal rules as a panel.
    pub(crate) fn mount_external(
        &mut self,
        panel_id: &str,
        location: ferris_sdk::ui::PanelLocation,
        document: SurfaceDocument,
    ) -> Result<InstanceId, String> {
        let contract = self.contract_for(panel_id)?;
        let instance = self
            .engine
            .admit(contract.clone())
            .map_err(|error| error.to_string())?;
        self.engine
            .mount(instance, document, &self.limits)
            .map_err(|error| {
                let _ = self.engine.dispose(instance);
                error.to_string()
            })?;
        self.mounted.insert(
            panel_id.to_string(),
            MountedPanel {
                instance,
                slot: HostSlot::from_panel_location(location),
                contract,
            },
        );
        Ok(instance)
    }

    fn contract_for(&self, panel_id: &str) -> Result<SurfaceContract, String> {
        let surface =
            SurfaceId::new("bsh.panel", panel_id.to_string()).map_err(|error| error.to_string())?;
        Ok(SurfaceContract::new(surface, Version::new(1, 0, 0))
            .with_profile(Profile::Full)
            .with_profile(Profile::Text)
            .with_state(StateModel::OnDemand)
            .with_isolation(IsolationClass::InProcess))
    }

    fn admit(
        &mut self,
        panel_id: &str,
        admission: &PanelAdmission<'_>,
    ) -> Result<InstanceId, String> {
        let contract = self.contract_for(panel_id)?;
        let document = match admission.document {
            Some(view) => SurfaceDocument::from_ui(view.clone()),
            None => empty_document(),
        };
        let instance = self
            .engine
            .admit(contract)
            .map_err(|error| error.to_string())?;
        self.engine
            .mount(instance, document, &self.limits)
            .map_err(|error| {
                let _ = self.engine.dispose(instance);
                error.to_string()
            })?;
        Ok(instance)
    }
}

/// Prefix every control action in a subtree with the panel that owns it.
fn prefix_actions(value: &mut serde_json::Value, prefix: &str) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(action)) = map.get_mut("action") {
                *action = format!("{prefix}{action}");
            }
            for child in map.values_mut() {
                prefix_actions(child, prefix);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                prefix_actions(item, prefix);
            }
        }
        _ => {}
    }
}

fn empty_document() -> SurfaceDocument {
    SurfaceDocument {
        schema_version: ferris_sdk::surface::document::LEGACY_SCHEMA_VERSION.to_string(),
        bindings: BTreeMap::new(),
        hidden_actions: Vec::new(),
        root: SurfaceNode::Column {
            id: None,
            children: Vec::new(),
        },
    }
}

/// The revision a mounted panel is at, for diagnostics.
pub(crate) fn revision_of(host: &SurfaceHost, panel_id: &str) -> Option<Revision> {
    let instance = host.engine().instance(host.instance_of(panel_id)?).ok()?;
    Some(instance.revision())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferris_sdk::ui::{ButtonStyle, PanelLocation, Tone, UiDocument, UiNode};

    fn view() -> UiDocument {
        UiDocument {
            schema_version: "1.0".to_string(),
            root: UiNode::Column {
                children: vec![UiNode::Text {
                    text: "status".to_string(),
                    tone: Tone::Neutral,
                }],
            },
        }
    }

    #[test]
    fn panels_become_instances_in_their_slot() {
        let mut host = SurfaceHost::new();
        let right = view();
        let bottom = view();
        let report = host.sync(&[
            PanelAdmission {
                panel_id: "com.example.right",
                location: PanelLocation::Right,
                document: Some(&right),
            },
            PanelAdmission {
                panel_id: "com.example.bottom",
                location: PanelLocation::Bottom,
                document: Some(&bottom),
            },
        ]);
        assert_eq!(report.mounted, 2);
        assert!(report.errors.is_empty());
        assert_eq!(host.len(), 2);
        assert_eq!(host.slot_of("com.example.right"), Some(HostSlot::Right));
        assert_eq!(host.slot_of("com.example.bottom"), Some(HostSlot::Bottom));
        assert_eq!(
            revision_of(&host, "com.example.right"),
            Some(Revision::initial())
        );
    }

    #[test]
    fn a_gone_panel_is_disposed_exactly_once() {
        let mut host = SurfaceHost::new();
        let right = view();
        host.sync(&[PanelAdmission {
            panel_id: "com.example.right",
            location: PanelLocation::Right,
            document: Some(&right),
        }]);
        let report = host.sync(&[]);
        assert_eq!(report.disposed, 1);
        assert_eq!(host.len(), 0);
        let again = host.sync(&[]);
        assert_eq!(again.disposed, 0);
    }

    #[test]
    fn a_bad_document_is_refused_without_taking_the_list_down() {
        let mut host = SurfaceHost::new();
        let bad = UiDocument {
            schema_version: "1.0".to_string(),
            root: UiNode::Button {
                label: "Go".to_string(),
                action: "not a contribution id".to_string(),
                style: ButtonStyle::Primary,
            },
        };
        let good = view();
        let report = host.sync(&[
            PanelAdmission {
                panel_id: "com.example.bad",
                location: PanelLocation::Right,
                document: Some(&bad),
            },
            PanelAdmission {
                panel_id: "com.example.good",
                location: PanelLocation::Left,
                document: Some(&good),
            },
        ]);
        assert_eq!(report.mounted, 1);
        assert_eq!(report.errors.len(), 1);
        assert!(host.refusal("com.example.bad").is_some());
        assert!(host.instance_of("com.example.bad").is_none());
        assert!(host.instance_of("com.example.good").is_some());
    }

    #[test]
    fn a_renewed_panel_gets_a_new_generation() {
        let mut host = SurfaceHost::new();
        let view = view();
        host.sync(&[PanelAdmission {
            panel_id: "com.example.right",
            location: PanelLocation::Right,
            document: Some(&view),
        }]);
        assert_eq!(host.generation_of("com.example.right"), Some(1));
        host.renew("com.example.right").expect("renew");
        assert_eq!(host.generation_of("com.example.right"), Some(2));
        assert_eq!(
            revision_of(&host, "com.example.right"),
            Some(Revision::initial())
        );
        let report = host.sync(&[]);
        assert_eq!(report.disposed, 1);
    }

    fn surface_document(root: SurfaceNode) -> SurfaceDocument {
        SurfaceDocument {
            schema_version: ferris_sdk::surface::SURFACE_SCHEMA_VERSION.to_string(),
            bindings: BTreeMap::new(),
            hidden_actions: Vec::new(),
            root,
        }
    }

    fn text(value: &str) -> SurfaceNode {
        SurfaceNode::Text {
            id: None,
            text: value.to_string(),
            tone: ferris_sdk::ui::Tone::Neutral,
        }
    }

    fn slot(surface: &str, fallback: SurfaceNode) -> SurfaceNode {
        SurfaceNode::Slot {
            id: None,
            surface: surface.to_string(),
            version: None,
            required: true,
            props: serde_json::Value::Null,
            sizing: ferris_sdk::surface::Sizing::default(),
            fallback: Some(Box::new(fallback)),
        }
    }

    fn rendered(host: &SurfaceHost, panel_id: &str) -> String {
        let (document, _) = host.view(panel_id).expect("a mounted view");
        serde_json::to_string(&document.to_ui_document().root).expect("json")
    }

    #[test]
    fn a_slot_draws_the_resolved_child_surface() {
        let mut host = SurfaceHost::new();
        host.mount_external(
            "com.example.child",
            PanelLocation::Right,
            surface_document(text("child content")),
        )
        .expect("child");
        host.mount_external(
            "com.example.parent",
            PanelLocation::Left,
            surface_document(SurfaceNode::Column {
                id: None,
                children: vec![slot("bsh.panel/com.example.child", text("missing"))],
            }),
        )
        .expect("parent");
        let json = rendered(&host, "com.example.parent");
        assert!(json.contains("child content"), "{json}");
        assert!(!json.contains("missing"), "{json}");
        assert!(!json.contains("\"slot\""), "{json}");
    }

    #[test]
    fn a_missing_child_keeps_the_declared_fallback() {
        let mut host = SurfaceHost::new();
        host.mount_external(
            "com.example.parent",
            PanelLocation::Left,
            surface_document(slot("bsh.panel/com.example.absent", text("fallback text"))),
        )
        .expect("parent");
        assert!(rendered(&host, "com.example.parent").contains("fallback text"));
    }

    #[test]
    fn a_cyclic_slot_chain_terminates_on_the_fallback() {
        let mut host = SurfaceHost::new();
        host.mount_external(
            "com.example.a",
            PanelLocation::Left,
            surface_document(slot("bsh.panel/com.example.b", text("a fallback"))),
        )
        .expect("a");
        host.mount_external(
            "com.example.b",
            PanelLocation::Right,
            surface_document(slot("bsh.panel/com.example.a", text("b fallback"))),
        )
        .expect("b");
        let json = rendered(&host, "com.example.a");
        assert!(json.contains("b fallback"), "{json}");
        assert!(!json.contains("a fallback"), "{json}");
    }

    #[test]
    fn a_transport_frame_advances_the_instance_and_close_disposes_it() {
        use ferris_sdk::surface::{Generation, SurfaceFrame};
        let mut host = SurfaceHost::new();
        let instance = host
            .mount_external(
                "com.example.remote",
                PanelLocation::Right,
                surface_document(text("one")),
            )
            .expect("mount");
        let frame = SurfaceFrame::full(
            instance,
            Generation::initial(),
            Revision::new(2),
            surface_document(text("two")),
        );
        assert_eq!(host.apply_frame(&frame), Ok(Revision::new(2)));
        let stale = SurfaceFrame::full(
            instance,
            Generation::initial(),
            Revision::new(2),
            surface_document(text("three")),
        );
        assert!(host.apply_frame(&stale).is_err());
        assert!(host.close_instance(instance));
        assert!(!host.close_instance(instance));
    }

    #[test]
    fn a_child_control_is_prefixed_with_its_panel() {
        let mut host = SurfaceHost::new();
        let legacy = ferris_sdk::ui::UiDocument {
            schema_version: "1.0".to_string(),
            root: ferris_sdk::ui::UiNode::Button {
                label: "Go".to_string(),
                action: "demo.go".to_string(),
                style: ferris_sdk::ui::ButtonStyle::Primary,
            },
        };
        host.mount_external(
            "com.example.child",
            PanelLocation::Right,
            SurfaceDocument::from_ui(legacy),
        )
        .expect("child");
        host.mount_external(
            "com.example.parent",
            PanelLocation::Left,
            surface_document(slot("bsh.panel/com.example.child", text("missing"))),
        )
        .expect("parent");
        let (document, _) = host.view("com.example.parent").expect("view");
        match document.root {
            SurfaceNode::Button { action, .. } => {
                assert_eq!(action, format!("com.example.child\u{1f}demo.go"));
            }
            other => panic!("expected the child button, got {other:?}"),
        }
    }

    #[test]
    fn rows_name_the_slot_and_the_generation() {
        let mut host = SurfaceHost::new();
        host.mount_external(
            "com.example.one",
            PanelLocation::Bottom,
            surface_document(text("one")),
        )
        .expect("one");
        let rows = host.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].panel_id, "com.example.one");
        assert_eq!(rows[0].slot, "workspace.bottom");
        assert_eq!(rows[0].generation, 1);
        assert_eq!(rows[0].revision, 1);
        assert!(rows[0].refused.is_none());
    }

    #[test]
    fn slot_rules_match_the_catalogue() {
        assert_eq!(HostSlot::Main.id(), "workspace.main");
        assert_eq!(HostSlot::Main.occupancy(), Occupancy::Single);
        assert!(HostSlot::Main.accepts_agents());
        assert!(HostSlot::SettingsPage.first_party_only());
        assert!(!HostSlot::SettingsPage.accepts_agents());
        assert_eq!(HostSlot::Right.occupancy(), Occupancy::Stacked);
    }
}
