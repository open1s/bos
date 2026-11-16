//! Instance ownership: identity, generation, revision and refusal rules.
//!
//! The engine is the sync state machine for one surface instance. It is not a
//! second runtime: the execution lifecycle of a provider belongs to the
//! runtime that hosts it (cordis-rs), and this engine holds only what a
//! renderer needs to decide whether a frame, patch or action is current.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::document::{DispatchPrincipal, SurfaceDocument};
use super::frame::{apply_patch, SurfaceFrame};
use super::{Generation, InstanceId, Revision, SurfaceContract, SurfaceError, SurfaceLimits};

/// The lifecycle state of one mounted surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstanceState {
    /// Admitted; no frame yet.
    Admitted,
    /// A required slot dependency is unsatisfied.
    Pending,
    /// A first valid frame is mounted.
    Mounted,
    /// Frames and actions flow.
    Live,
    /// A dependency vanished or a frame was refused; the last frame stays.
    Degraded,
    /// Teardown started.
    Unmounting,
    /// Teardown finished; the instance cannot be revived.
    Disposed,
}

impl InstanceState {
    /// The stable lower-case name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::Pending => "pending",
            Self::Mounted => "mounted",
            Self::Live => "live",
            Self::Degraded => "degraded",
            Self::Unmounting => "unmounting",
            Self::Disposed => "disposed",
        }
    }

    /// Whether the instance can receive a frame.
    pub fn accepts_frames(self) -> bool {
        matches!(
            self,
            Self::Admitted | Self::Pending | Self::Mounted | Self::Live | Self::Degraded
        )
    }

    /// Whether the instance can dispatch an action.
    pub fn accepts_actions(self) -> bool {
        matches!(self, Self::Mounted | Self::Live)
    }
}

impl fmt::Display for InstanceState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One live surface instance.
#[derive(Debug, Clone)]
pub struct SurfaceInstance {
    id: InstanceId,
    contract: SurfaceContract,
    generation: Generation,
    revision: Revision,
    state: InstanceState,
    document: Option<SurfaceDocument>,
    last_good: Option<SurfaceDocument>,
    props: Value,
    pending_reason: Option<String>,
}

impl SurfaceInstance {
    /// The identity.
    pub fn id(&self) -> InstanceId {
        self.id
    }

    /// The provider contract.
    pub fn contract(&self) -> &SurfaceContract {
        &self.contract
    }

    /// The current generation.
    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// The current revision.
    pub fn revision(&self) -> Revision {
        self.revision
    }

    /// The lifecycle state.
    pub fn state(&self) -> InstanceState {
        self.state
    }

    /// The current document, when a frame has been mounted.
    pub fn document(&self) -> Option<&SurfaceDocument> {
        self.document.as_ref()
    }

    /// The last document that passed validation.
    pub fn last_good(&self) -> Option<&SurfaceDocument> {
        self.last_good.as_ref()
    }

    /// The mount props the parent supplied.
    pub fn props(&self) -> &Value {
        &self.props
    }

    /// Why a required dependency left the instance Pending.
    pub fn pending_reason(&self) -> Option<&str> {
        self.pending_reason.as_deref()
    }
}

/// One action request, materialized and ready for the host to authorize.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionCall {
    /// Provider-qualified capability.
    pub capability: String,
    /// Optional named operation.
    pub operation: Option<String>,
    /// The principal that must perform the call.
    pub principal: DispatchPrincipal,
    /// The mapped input.
    pub input: Value,
}

/// The event a control reported.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceEvent {
    /// Submitted text.
    pub value: Option<String>,
    /// Checkbox state.
    pub checked: Option<bool>,
    /// Selected option value.
    pub option: Option<String>,
}

/// What a disposal released.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceDisposalReport {
    /// The disposed instance.
    pub instance: InstanceId,
    /// The generation that was released.
    pub generation: Generation,
    /// Whether the instance was already disposed before this call.
    pub already_disposed: bool,
}

/// The owner of every mounted surface instance in one scope.
#[derive(Debug, Default)]
pub struct SurfaceEngine {
    instances: BTreeMap<InstanceId, SurfaceInstance>,
}

impl SurfaceEngine {
    /// An empty engine.
    pub fn new() -> Self {
        Self::default()
    }

    /// Admit a validated provider contract and return its instance identity.
    pub fn admit(&mut self, contract: SurfaceContract) -> Result<InstanceId, SurfaceError> {
        contract.ensure_valid()?;
        let id = InstanceId::new();
        self.instances.insert(
            id,
            SurfaceInstance {
                id,
                contract,
                generation: Generation::initial(),
                revision: Revision::zero(),
                state: InstanceState::Admitted,
                document: None,
                last_good: None,
                props: Value::Null,
                pending_reason: None,
            },
        );
        Ok(id)
    }

    /// Mount a first document.
    pub fn mount(
        &mut self,
        id: InstanceId,
        document: SurfaceDocument,
        limits: &SurfaceLimits,
    ) -> Result<Revision, SurfaceError> {
        self.mount_with_props(id, document, Value::Null, limits)
    }

    /// Mount a first document with the props the parent supplied.
    pub fn mount_with_props(
        &mut self,
        id: InstanceId,
        document: SurfaceDocument,
        props: Value,
        limits: &SurfaceLimits,
    ) -> Result<Revision, SurfaceError> {
        let instance = self.instance_mut(id)?;
        if instance.state == InstanceState::Disposed {
            return Err(SurfaceError::Disposed);
        }
        if !matches!(
            instance.state,
            InstanceState::Admitted | InstanceState::Pending
        ) {
            return Err(SurfaceError::IllegalState {
                instance: id.to_string(),
                state: instance.state.to_string(),
                operation: "mount a first frame".to_string(),
            });
        }
        let report = document.validate_against(&instance.contract, limits);
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
        let revision = Revision::initial();
        instance.document = Some(document.clone());
        instance.last_good = Some(document);
        instance.props = props;
        instance.revision = revision;
        instance.state = InstanceState::Mounted;
        instance.pending_reason = None;
        Ok(revision)
    }

    /// Apply a full frame or a base-relative patch.
    pub fn apply_frame(
        &mut self,
        frame: &SurfaceFrame,
        limits: &SurfaceLimits,
    ) -> Result<Revision, SurfaceError> {
        let instance = self.instance_mut(frame.instance)?;
        if instance.state == InstanceState::Disposed {
            return Err(SurfaceError::Disposed);
        }
        if instance.generation != frame.generation {
            return Err(SurfaceError::StaleGeneration {
                found: frame.generation.value(),
                current: instance.generation.value(),
            });
        }
        if !instance.state.accepts_frames() {
            return Err(SurfaceError::IllegalState {
                instance: frame.instance.to_string(),
                state: instance.state.to_string(),
                operation: "apply a frame".to_string(),
            });
        }
        if frame.revision <= instance.revision {
            return Err(SurfaceError::StaleRevision {
                found: frame.revision.value(),
                current: instance.revision.value(),
            });
        }
        let document = match (&frame.document, frame.base_revision) {
            (Some(document), None) => document.clone(),
            (None, Some(base)) => {
                if base != instance.revision {
                    return Err(SurfaceError::StaleRevision {
                        found: base.value(),
                        current: instance.revision.value(),
                    });
                }
                let current =
                    instance
                        .document
                        .as_ref()
                        .ok_or_else(|| SurfaceError::IllegalState {
                            instance: frame.instance.to_string(),
                            state: instance.state.to_string(),
                            operation: "patch before the first frame".to_string(),
                        })?;
                apply_patch(current, &frame.patch, limits)?
            }
            _ => {
                return Err(SurfaceError::Invalid(
                    "a frame carries either a document or a patch base, not both or neither"
                        .to_string(),
                ))
            }
        };
        let report = document.validate_against(&instance.contract, limits);
        if !report.is_valid() {
            instance.state = InstanceState::Degraded;
            instance.pending_reason = Some("a frame failed validation".to_string());
            return Err(SurfaceError::Invalid(
                report
                    .errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; "),
            ));
        }
        instance.document = Some(document.clone());
        instance.last_good = Some(document);
        instance.revision = frame.revision;
        instance.state = InstanceState::Live;
        instance.pending_reason = None;
        Ok(frame.revision)
    }

    /// Mark a required dependency unsatisfied while keeping the last frame.
    pub fn suspend(
        &mut self,
        id: InstanceId,
        reason: impl Into<String>,
    ) -> Result<(), SurfaceError> {
        let instance = self.instance_mut(id)?;
        if instance.state == InstanceState::Disposed {
            return Err(SurfaceError::Disposed);
        }
        instance.state = InstanceState::Pending;
        instance.pending_reason = Some(reason.into());
        Ok(())
    }

    /// Mark a dependency vanished, keeping the last good frame visible.
    pub fn degrade(
        &mut self,
        id: InstanceId,
        reason: impl Into<String>,
    ) -> Result<(), SurfaceError> {
        let instance = self.instance_mut(id)?;
        if instance.state == InstanceState::Disposed {
            return Err(SurfaceError::Disposed);
        }
        instance.state = InstanceState::Degraded;
        instance.pending_reason = Some(reason.into());
        Ok(())
    }

    /// Replace the provider: a new generation, a new first frame.
    pub fn replace(
        &mut self,
        id: InstanceId,
        contract: SurfaceContract,
        document: SurfaceDocument,
        limits: &SurfaceLimits,
    ) -> Result<Generation, SurfaceError> {
        contract.ensure_valid()?;
        {
            let instance = self.instance(id)?;
            if instance.state == InstanceState::Disposed {
                return Err(SurfaceError::Disposed);
            }
        }
        let report = document.validate_against(&contract, limits);
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
        let instance = self.instance_mut(id)?;
        instance.generation = instance.generation.next();
        instance.revision = Revision::initial();
        instance.contract = contract;
        instance.document = Some(document.clone());
        instance.last_good = Some(document);
        instance.state = InstanceState::Mounted;
        instance.pending_reason = None;
        Ok(instance.generation)
    }

    /// Dispose an instance. Idempotent.
    pub fn dispose(&mut self, id: InstanceId) -> Result<SurfaceDisposalReport, SurfaceError> {
        let instance = self.instance_mut(id)?;
        let already_disposed = instance.state == InstanceState::Disposed;
        instance.generation = if instance.generation.value() == 0 {
            Generation::initial()
        } else {
            instance.generation
        };
        instance.state = InstanceState::Disposed;
        instance.document = None;
        instance.last_good = None;
        instance.pending_reason = Some("disposed".to_string());
        Ok(SurfaceDisposalReport {
            instance: id,
            generation: instance.generation,
            already_disposed,
        })
    }

    /// Resolve one action into a call ready for authorization.
    pub fn dispatch(
        &self,
        id: InstanceId,
        generation: Generation,
        revision: Revision,
        binding: &str,
        event: &SurfaceEvent,
    ) -> Result<ActionCall, SurfaceError> {
        let instance = self.instance(id)?;
        if instance.state == InstanceState::Disposed {
            return Err(SurfaceError::Disposed);
        }
        if !instance.state.accepts_actions() {
            return Err(SurfaceError::IllegalState {
                instance: id.to_string(),
                state: instance.state.to_string(),
                operation: "dispatch an action".to_string(),
            });
        }
        if instance.generation != generation {
            return Err(SurfaceError::StaleGeneration {
                found: generation.value(),
                current: instance.generation.value(),
            });
        }
        if instance.revision != revision {
            return Err(SurfaceError::StaleRevision {
                found: revision.value(),
                current: instance.revision.value(),
            });
        }
        let document = instance
            .document
            .as_ref()
            .ok_or_else(|| SurfaceError::IllegalState {
                instance: id.to_string(),
                state: instance.state.to_string(),
                operation: "dispatch before a frame".to_string(),
            })?;
        let binding = document
            .binding(binding)
            .ok_or_else(|| SurfaceError::UnknownBinding(binding.to_string()))?;
        let input = materialize(&binding.input, event, &instance.props)?;
        Ok(ActionCall {
            capability: binding.capability.clone(),
            operation: binding.operation.clone(),
            principal: binding.principal,
            input,
        })
    }

    /// Look up one instance.
    pub fn instance(&self, id: InstanceId) -> Result<&SurfaceInstance, SurfaceError> {
        self.instances
            .get(&id)
            .ok_or_else(|| SurfaceError::UnknownInstance(id.to_string()))
    }

    /// Every instance, in identity order.
    pub fn instances(&self) -> impl Iterator<Item = &SurfaceInstance> {
        self.instances.values()
    }

    /// How many instances the engine owns.
    pub fn len(&self) -> usize {
        self.instances.len()
    }

    /// Whether the engine owns no instance.
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    fn instance_mut(&mut self, id: InstanceId) -> Result<&mut SurfaceInstance, SurfaceError> {
        self.instances
            .get_mut(&id)
            .ok_or_else(|| SurfaceError::UnknownInstance(id.to_string()))
    }
}

fn materialize(input: &Value, event: &SurfaceEvent, props: &Value) -> Result<Value, SurfaceError> {
    match input {
        Value::String(text) if text == "$event.value" => {
            Ok(Value::String(event.value.clone().unwrap_or_default()))
        }
        Value::String(text) if text == "$event.checked" => {
            Ok(Value::Bool(event.checked.unwrap_or(false)))
        }
        Value::String(text) if text == "$event.option" => {
            Ok(Value::String(event.option.clone().unwrap_or_default()))
        }
        Value::String(text) if text.starts_with("$props.") => {
            let name = text.trim_start_matches("$props.");
            Ok(props.get(name).cloned().unwrap_or(Value::Null))
        }
        Value::String(text) if text.starts_with('$') => Err(SurfaceError::Invalid(format!(
            "input form {text:?} is not a supported mapping"
        ))),
        Value::Array(items) => items
            .iter()
            .map(|item| materialize(item, event, props))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, value) in map {
                out.insert(key.clone(), materialize(value, event, props)?);
            }
            Ok(Value::Object(out))
        }
        other => Ok(other.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::contract::{ActionSpec, Profile};
    use crate::surface::document::{ActionBinding, ActionMode};
    use crate::surface::node::SurfaceNode;
    use crate::surface::{SurfaceId, SURFACE_SCHEMA_VERSION};
    use crate::Risk;

    fn contract() -> SurfaceContract {
        SurfaceContract::new(
            SurfaceId::parse("acme.git/status").expect("id"),
            semver::Version::new(1, 2, 3),
        )
        .with_profile(Profile::Text)
        .with_action(ActionSpec {
            id: "refresh".to_string(),
            label: "Refresh".to_string(),
            capability: "acme.git/refresh".to_string(),
            risk: Risk::Low,
            confirmation: super::super::Confirmation::Inherit,
        })
    }

    fn document() -> SurfaceDocument {
        let mut bindings = BTreeMap::new();
        bindings.insert(
            "refresh".to_string(),
            ActionBinding {
                capability: "acme.git/refresh".to_string(),
                operation: None,
                input: serde_json::json!({"repo": "$props.repo"}),
                principal: DispatchPrincipal::Producer,
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
                children: vec![SurfaceNode::Button {
                    id: "refresh".to_string(),
                    label: "Refresh".to_string(),
                    action: "refresh".to_string(),
                    style: crate::ui::ButtonStyle::Primary,
                }],
            },
        }
    }

    #[test]
    fn a_frame_must_move_forward() {
        let mut engine = SurfaceEngine::new();
        let id = engine.admit(contract()).expect("admit");
        engine
            .mount_with_props(
                id,
                document(),
                serde_json::json!({"repo": "/work"}),
                &SurfaceLimits::default(),
            )
            .expect("mount");
        let stale = SurfaceFrame::full(id, Generation::initial(), Revision::initial(), document());
        assert!(matches!(
            engine.apply_frame(&stale, &SurfaceLimits::default()),
            Err(SurfaceError::StaleRevision { .. })
        ));
        let next = SurfaceFrame::full(id, Generation::initial(), Revision(2), document());
        assert_eq!(
            engine.apply_frame(&next, &SurfaceLimits::default()),
            Ok(Revision(2))
        );
    }

    #[test]
    fn a_replaced_generation_cannot_write() {
        let mut engine = SurfaceEngine::new();
        let id = engine.admit(contract()).expect("admit");
        engine
            .mount(id, document(), &SurfaceLimits::default())
            .expect("mount");
        let generation = engine
            .replace(id, contract(), document(), &SurfaceLimits::default())
            .expect("replace");
        assert_eq!(generation, Generation::initial().next());
        let old = SurfaceFrame::full(id, Generation::initial(), Revision(2), document());
        assert!(matches!(
            engine.apply_frame(&old, &SurfaceLimits::default()),
            Err(SurfaceError::StaleGeneration { .. })
        ));
    }

    #[test]
    fn an_action_maps_props_and_refuses_a_stale_revision() {
        let mut engine = SurfaceEngine::new();
        let id = engine.admit(contract()).expect("admit");
        let revision = engine
            .mount_with_props(
                id,
                document(),
                serde_json::json!({"repo": "/work"}),
                &SurfaceLimits::default(),
            )
            .expect("mount");
        let call = engine
            .dispatch(
                id,
                Generation::initial(),
                revision,
                "refresh",
                &SurfaceEvent::default(),
            )
            .expect("dispatch");
        assert_eq!(call.capability, "acme.git/refresh");
        assert_eq!(call.input, serde_json::json!({"repo": "/work"}));
        assert!(matches!(
            engine.dispatch(
                id,
                Generation::initial(),
                Revision(99),
                "refresh",
                &SurfaceEvent::default()
            ),
            Err(SurfaceError::StaleRevision { .. })
        ));
        assert!(matches!(
            engine.dispatch(
                id,
                Generation::initial(),
                revision,
                "missing",
                &SurfaceEvent::default()
            ),
            Err(SurfaceError::UnknownBinding(_))
        ));
    }

    #[test]
    fn dispose_is_idempotent_and_blocks_actions() {
        let mut engine = SurfaceEngine::new();
        let id = engine.admit(contract()).expect("admit");
        let revision = engine
            .mount(id, document(), &SurfaceLimits::default())
            .expect("mount");
        let first = engine.dispose(id).expect("dispose");
        assert!(!first.already_disposed);
        let second = engine.dispose(id).expect("dispose again");
        assert!(second.already_disposed);
        assert!(matches!(
            engine.dispatch(
                id,
                Generation::initial(),
                revision,
                "refresh",
                &SurfaceEvent::default()
            ),
            Err(SurfaceError::Disposed)
        ));
    }

    #[test]
    fn a_refused_frame_keeps_the_last_good_document() {
        let mut engine = SurfaceEngine::new();
        let id = engine.admit(contract()).expect("admit");
        engine
            .mount(id, document(), &SurfaceLimits::default())
            .expect("mount");
        let mut broken = document();
        broken.bindings.clear();
        let frame = SurfaceFrame::full(id, Generation::initial(), Revision(2), broken);
        assert!(engine
            .apply_frame(&frame, &SurfaceLimits::default())
            .is_err());
        let instance = engine.instance(id).expect("instance");
        assert_eq!(instance.state(), InstanceState::Degraded);
        assert!(instance.last_good().is_some());
        assert!(instance
            .last_good()
            .expect("last good")
            .binding("refresh")
            .is_some());
    }
}
