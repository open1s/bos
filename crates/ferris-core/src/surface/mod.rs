//! Dynamic UI surface contract.
//!
//! A surface is a renderable capability: a provider-qualified, versioned,
//! generation-owned region with declared actions, slots and guarantees. This
//! module is host neutral. It carries no GPUI, BSH or renderer type, so the
//! same contract can be answered by a native panel, a declarative package
//! view, a Cordis-compatible runtime, a process or WASM guest, a remote BOS
//! node, or an agent turn.
//!
//! The layers are deliberately separate:
//!
//! - [node] is the renderable tree: the schema 1.1 node vocabulary, stable
//!   node ids, and the slot node that composes another surface.
//! - [document] is a validated tree plus its action binding table.
//! - [contract] describes what a provider offers and promises.
//! - [frame] carries a full document or a base-relative patch.
//! - [compose] resolves slots against a catalogue of providers.
//! - [engine] owns instance identity, generations, revisions, refusal rules
//!   and disposal.
//!
//! Rendering, focus, layout and authorization stay with the host. Authoring
//! is the provider's job. No document may grant a capability.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub mod compose;
pub mod contract;
pub mod document;
pub mod engine;
pub mod frame;
pub mod node;
pub mod runtime;
pub mod wire;

pub use compose::{CompositionPlan, SlotMount, SurfaceCatalog, SurfaceRequirement, UnresolvedSlot};
pub use contract::{
    ActionSpec, Confirmation, IsolationClass, Profile, ProviderGuarantees, SlotSpec, StateModel,
    SurfaceContract, SurfaceLimitsSpec,
};
pub use document::{
    ActionBinding, ActionMode, DispatchPrincipal, HiddenAction, SurfaceDocument,
    SURFACE_SCHEMA_VERSION,
};
pub use engine::{
    ActionCall, InstanceState, SurfaceDisposalReport, SurfaceEngine, SurfaceEvent, SurfaceInstance,
};
pub use frame::{PatchOp, SurfaceFrame};
pub use node::{Overflow, Sizing, SurfaceNode, SurfaceTab};
pub use runtime::{SurfaceProvider, SurfaceRequest};
pub use wire::{
    decode, decode_with, encode, encode_with, SurfaceAction, SurfaceEnvelope, SurfaceMessage,
    WireLimits, MAX_WIRE_BYTES, SURFACE_WIRE_VERSION,
};

/// The contract family every surface answers.
pub const SURFACE_CONTRACT: &str = "bos.ui.surface";

/// A provider-qualified surface identity: provider and provider-local name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SurfaceId {
    provider: String,
    name: String,
}

impl SurfaceId {
    /// Build an id from a provider and a provider-local name.
    pub fn new(provider: impl Into<String>, name: impl Into<String>) -> Result<Self, SurfaceError> {
        let provider = provider.into();
        let name = name.into();
        validate_part("provider", &provider)?;
        validate_part("name", &name)?;
        Ok(Self { provider, name })
    }

    /// Parse the wire form: provider slash name.
    pub fn parse(raw: impl Into<String>) -> Result<Self, SurfaceError> {
        let raw = raw.into();
        let Some((provider, name)) = raw.split_once('/') else {
            return Err(SurfaceError::InvalidId {
                value: raw,
                reason: "a surface id is provider-qualified: <provider>/<name>".to_string(),
            });
        };
        Self::new(provider, name)
    }

    /// The providing principal.
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// The provider-local name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for SurfaceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.provider, self.name)
    }
}

impl FromStr for SurfaceId {
    type Err = SurfaceError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for SurfaceId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for SurfaceId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        SurfaceId::parse(raw).map_err(serde::de::Error::custom)
    }
}

fn validate_part(what: &str, value: &str) -> Result<(), SurfaceError> {
    if value.is_empty() {
        return Err(SurfaceError::InvalidId {
            value: value.to_string(),
            reason: format!("the {what} may not be empty"),
        });
    }
    if value.contains('/') {
        return Err(SurfaceError::InvalidId {
            value: value.to_string(),
            reason: "a slash separates provider and name and may not appear inside either"
                .to_string(),
        });
    }
    if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(SurfaceError::InvalidId {
            value: value.to_string(),
            reason: "whitespace and control characters are not allowed".to_string(),
        });
    }
    Ok(())
}

/// One mounted surface occurrence inside a host scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstanceId(Uuid);

impl InstanceId {
    /// Create a fresh instance identity.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// The identity as a display string.
    pub fn as_str(&self) -> String {
        self.0.to_string()
    }
}

impl Default for InstanceId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for InstanceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl Serialize for InstanceId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for InstanceId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Uuid::parse_str(&raw)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

/// The provider generation of one instance.
///
/// A replacement increments it. Every frame, patch and action names the
/// generation it was produced for, so a stale producer cannot write into the
/// generation that replaced it.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(transparent)]
pub struct Generation(u64);

impl Generation {
    /// A generation from its numeric value, for decoding a transport message.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// The first generation of an admitted instance.
    pub fn initial() -> Self {
        Self(1)
    }

    /// The numeric value.
    pub fn value(self) -> u64 {
        self.0
    }

    /// The next generation.
    #[must_use]
    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

impl fmt::Display for Generation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// The monotonic revision of one instance within one generation.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(transparent)]
pub struct Revision(u64);

impl Revision {
    /// A revision from its numeric value, for decoding a transport message.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// Revision zero, before the first frame.
    pub fn zero() -> Self {
        Self(0)
    }

    /// The first frame of a generation.
    pub fn initial() -> Self {
        Self(1)
    }

    /// The numeric value.
    pub fn value(self) -> u64 {
        self.0
    }

    /// The next revision.
    #[must_use]
    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// Structural and operational ceilings applied to one surface document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceLimits {
    /// Maximum nesting depth of the node tree.
    pub max_depth: usize,
    /// Maximum node count.
    pub max_nodes: usize,
    /// Maximum characters in one text leaf.
    pub max_text: usize,
    /// Maximum characters across all text leaves.
    pub max_total_text: usize,
    /// Maximum serialized document size in bytes.
    pub max_bytes: usize,
    /// Maximum action bindings in one document.
    pub max_bindings: usize,
    /// Maximum operations in one patch.
    pub max_patch_ops: usize,
    /// Maximum serialized patch size in bytes.
    pub max_patch_bytes: usize,
    /// Maximum nested slot depth a composition may reach.
    pub max_slot_depth: usize,
    /// Maximum slots directly under one node.
    pub max_slot_fanout: usize,
    /// Maximum live surface instances in one scope.
    pub max_instances: usize,
}

impl Default for SurfaceLimits {
    fn default() -> Self {
        Self {
            max_depth: 32,
            max_nodes: 1000,
            max_text: 16 * 1024,
            max_total_text: 256 * 1024,
            max_bytes: 1024 * 1024,
            max_bindings: 64,
            max_patch_ops: 256,
            max_patch_bytes: 64 * 1024,
            max_slot_depth: 4,
            max_slot_fanout: 8,
            max_instances: 32,
        }
    }
}

impl SurfaceLimits {
    /// The narrower defaults applied to an agent-authored surface.
    pub fn agent() -> Self {
        Self {
            max_nodes: 120,
            max_bytes: 16 * 1024,
            ..Self::default()
        }
    }
}

/// Everything the surface framework can refuse.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SurfaceError {
    /// An identity could not be parsed.
    #[error("invalid surface id {value:?}: {reason}")]
    InvalidId {
        /// The rejected value.
        value: String,
        /// Why it was rejected.
        reason: String,
    },
    /// A document or contract failed validation.
    #[error("surface validation failed: {0}")]
    Invalid(String),
    /// The instance is unknown to this engine.
    #[error("unknown surface instance {0}")]
    UnknownInstance(String),
    /// The instance is disposed and cannot be revived.
    #[error("surface instance is disposed")]
    Disposed,
    /// The operation is not legal from the current state.
    #[error("instance {instance} is {state}; it cannot {operation}")]
    IllegalState {
        /// The instance.
        instance: String,
        /// The current state.
        state: String,
        /// The attempted operation.
        operation: String,
    },
    /// A frame, patch or action named an old revision.
    #[error("stale revision {found}; the instance is at {current}")]
    StaleRevision {
        /// The revision the producer named.
        found: u64,
        /// The current revision.
        current: u64,
    },
    /// A message came from a replaced generation.
    #[error("stale generation {found}; the instance is at {current}")]
    StaleGeneration {
        /// The named generation.
        found: u64,
        /// The current generation.
        current: u64,
    },
    /// A control named a binding the document does not carry.
    #[error("unknown action binding {0:?}")]
    UnknownBinding(String),
    /// A patch addressed a node id that does not exist.
    #[error("unknown node id {0:?}")]
    UnknownNode(String),
    /// No provider matches a slot.
    #[error("no provider for surface {0}")]
    NoProvider(String),
    /// More than one provider matches and the choice is not deterministic.
    #[error("ambiguous provider for surface {surface}: {candidates:?}")]
    AmbiguousProvider {
        /// The requested surface.
        surface: String,
        /// The candidates considered.
        candidates: Vec<String>,
    },
    /// A composition contains a cycle.
    #[error("slot cycle: {0}")]
    Cycle(String),
    /// A declared ceiling was exceeded.
    #[error("surface budget exceeded: {0}")]
    Budget(String),
}
