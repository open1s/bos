//! What a surface provider offers, and what it promises.
//!
//! A contract is declared once per provider surface. It is validated before
//! admission, and every document is checked against it at mount: a document
//! may only reach an action the contract advertised.

use std::collections::{BTreeMap, BTreeSet};

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::types::{ValidationIssue, ValidationReport};
use crate::Risk;

use super::{SurfaceError, SurfaceId, SurfaceLimits, SURFACE_CONTRACT};

/// A rendering affordance level the host can ask for.
///
/// The same surface is usable from a full native window, a narrow dock, a
/// terminal and another agent, so a provider declares which levels it can
/// actually honour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Profile {
    /// The full node vocabulary.
    Full,
    /// A reduced vocabulary for narrow regions.
    Compact,
    /// Text and tables only, with numbered actions.
    Text,
    /// Structured data with no layout, for an agent or a test harness.
    Machine,
}

impl Profile {
    /// The stable lower-case name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Compact => "compact",
            Self::Text => "text",
            Self::Machine => "machine",
        }
    }
}

/// How the host keeps a mounted region current.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StateModel {
    /// The shipped document is never re-read.
    #[default]
    Static,
    /// Read once per mount and on explicit refresh.
    OnDemand,
    /// Subscribe; frames and patches arrive as they happen.
    Stream,
    /// Fetch a bounded, cacheable projection.
    Snapshot,
}

/// The execution class behind a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IsolationClass {
    /// Native code in the host process.
    InProcess,
    /// A Cordis plugin running behind its runtime.
    NodeBridge,
    /// An extension process.
    Process,
    /// A WebAssembly component or Extism guest.
    Wasm,
    /// Another BOS node.
    Remote,
}

/// Whether an action requires a host confirmation before dispatch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Confirmation {
    /// The host derives confirmation from the capability risk.
    #[default]
    Inherit,
    /// Always confirm, even at low risk.
    Host,
}

/// One action a contract advertises.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionSpec {
    /// Stable action id, also the binding key a control names.
    pub id: String,
    /// Label the host may show, untrusted.
    #[serde(default)]
    pub label: String,
    /// Provider-qualified capability this action may reach.
    pub capability: String,
    /// Risk bucket, used by the host to decide on confirmation.
    pub risk: Risk,
    /// Explicit confirmation policy.
    #[serde(default)]
    pub confirmation: Confirmation,
}

/// One mount point a surface offers to other surfaces.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlotSpec {
    /// Stable slot name, unique inside the contract.
    pub name: String,
    /// What the slot is for.
    #[serde(default)]
    pub description: String,
    /// Whether the host may mount a surface there without the presenter.
    #[serde(default = "default_true")]
    pub open: bool,
}

/// What a provider can promise about updating, degrading and theming.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderGuarantees {
    /// Whether patches are supported and how many ops one may carry.
    #[serde(default)]
    pub patch: Option<u16>,
    /// Whether the provider streams frames.
    #[serde(default)]
    pub streaming: bool,
    /// Whether the last good frame may be shown while unreachable.
    #[serde(default)]
    pub offline: bool,
    /// Whether identical props and revision yield an identical document.
    #[serde(default)]
    pub deterministic: bool,
    /// Tone tokens the provider honours.
    #[serde(default)]
    pub themes: Vec<String>,
    /// Accessibility roles the provider promises for interactive nodes.
    #[serde(default)]
    pub accessible: bool,
    /// Whether controls may dispatch.
    #[serde(default = "default_true")]
    pub interactive: bool,
}

impl Default for ProviderGuarantees {
    fn default() -> Self {
        Self {
            patch: None,
            streaming: false,
            offline: false,
            deterministic: true,
            themes: Vec::new(),
            accessible: false,
            interactive: true,
        }
    }
}

/// The typed description of one surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SurfaceContract {
    /// Contract family, always the surface contract id.
    pub contract: String,
    /// Contract family version.
    pub contract_version: Version,
    /// Provider-qualified surface identity.
    pub surface: SurfaceId,
    /// Human-readable label, untrusted.
    #[serde(default)]
    pub display_name: String,
    /// Human-readable description, untrusted.
    #[serde(default)]
    pub description: String,
    /// Profiles the provider can render.
    pub profiles: BTreeSet<Profile>,
    /// How the host keeps the region current.
    #[serde(default)]
    pub state: StateModel,
    /// Actions this surface may bind.
    #[serde(default)]
    pub actions: Vec<ActionSpec>,
    /// Mount points this surface offers.
    #[serde(default)]
    pub slots: Vec<SlotSpec>,
    /// Per-surface limits.
    #[serde(default)]
    pub limits: SurfaceLimitsSpec,
    /// What the provider promises.
    #[serde(default)]
    pub guarantees: ProviderGuarantees,
    /// Execution class.
    pub isolation: IsolationClass,
}

/// Limits as they appear in a contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceLimitsSpec {
    /// Maximum nodes in one frame.
    pub max_nodes: usize,
    /// Maximum serialized bytes in one frame.
    pub max_bytes: usize,
}

impl Default for SurfaceLimitsSpec {
    fn default() -> Self {
        let limits = SurfaceLimits::default();
        Self {
            max_nodes: limits.max_nodes,
            max_bytes: limits.max_bytes,
        }
    }
}

impl SurfaceContract {
    /// Build a contract with the safe defaults.
    pub fn new(surface: SurfaceId, contract_version: Version) -> Self {
        Self {
            contract: SURFACE_CONTRACT.to_string(),
            contract_version,
            surface,
            display_name: String::new(),
            description: String::new(),
            profiles: [Profile::Full].into_iter().collect(),
            state: StateModel::Static,
            actions: Vec::new(),
            slots: Vec::new(),
            limits: SurfaceLimitsSpec::default(),
            guarantees: ProviderGuarantees::default(),
            isolation: IsolationClass::InProcess,
        }
    }

    /// Add a supported profile.
    #[must_use]
    pub fn with_profile(mut self, profile: Profile) -> Self {
        self.profiles.insert(profile);
        self
    }

    /// Add an advertised action.
    #[must_use]
    pub fn with_action(mut self, action: ActionSpec) -> Self {
        self.actions.push(action);
        self
    }

    /// Add an offered slot.
    #[must_use]
    pub fn with_slot(mut self, slot: SlotSpec) -> Self {
        self.slots.push(slot);
        self
    }

    /// Set the state model.
    #[must_use]
    pub fn with_state(mut self, state: StateModel) -> Self {
        self.state = state;
        self
    }

    /// Set the execution class.
    #[must_use]
    pub fn with_isolation(mut self, isolation: IsolationClass) -> Self {
        self.isolation = isolation;
        self
    }

    /// Set the guarantees.
    #[must_use]
    pub fn with_guarantees(mut self, guarantees: ProviderGuarantees) -> Self {
        self.guarantees = guarantees;
        self
    }

    /// Look up an advertised action.
    pub fn action(&self, id: &str) -> Option<&ActionSpec> {
        self.actions.iter().find(|action| action.id == id)
    }

    /// Whether this provider can render a profile.
    pub fn supports(&self, profile: Profile) -> bool {
        self.profiles.contains(&profile)
    }

    /// Validate the declared contract.
    pub fn validate(&self) -> ValidationReport {
        let mut report = ValidationReport::default();
        if self.contract != SURFACE_CONTRACT {
            report.error(
                "surface.contract",
                format!(
                    "contract family {:?} is not {:?}",
                    self.contract, SURFACE_CONTRACT
                ),
            );
        }
        if self.profiles.is_empty() {
            report.error("surface.profiles", "a contract needs at least one profile");
        }
        if self.limits.max_nodes == 0 {
            report.error("surface.limits.max_nodes", "must be greater than zero");
        }
        if self.limits.max_bytes == 0 {
            report.error("surface.limits.max_bytes", "must be greater than zero");
        }
        let mut actions = BTreeSet::new();
        for action in &self.actions {
            if action.id.trim().is_empty() {
                report.error("surface.actions.id", "action ids must not be empty");
            } else if !actions.insert(action.id.clone()) {
                report.error(
                    "surface.actions.id",
                    format!("action {:?} is declared twice", action.id),
                );
            }
            if SurfaceId::parse(action.capability.clone()).is_err() {
                report.error(
                    "surface.actions.capability",
                    format!(
                        "action {:?} names {:?}, which is not provider-qualified",
                        action.id, action.capability
                    ),
                );
            }
        }
        let mut slots = BTreeSet::new();
        for slot in &self.slots {
            if slot.name.trim().is_empty() {
                report.error("surface.slots.name", "slot names must not be empty");
            } else if !slots.insert(slot.name.clone()) {
                report.error(
                    "surface.slots.name",
                    format!("slot {:?} is declared twice", slot.name),
                );
            }
        }
        if self.guarantees.patch == Some(0) {
            report.error(
                "surface.guarantees.patch",
                "a patch guarantee of zero ops is not a guarantee",
            );
        }
        report
    }

    /// Validate this contract and turn a failure into one error.
    pub fn ensure_valid(&self) -> Result<(), SurfaceError> {
        let report = self.validate();
        if report.is_valid() {
            return Ok(());
        }
        Err(SurfaceError::Invalid(errors(&report)))
    }

    /// The actions by id, for diagnostics and the catalogue.
    pub fn actions_by_id(&self) -> BTreeMap<&str, &ActionSpec> {
        self.actions
            .iter()
            .map(|action| (action.id.as_str(), action))
            .collect()
    }
}

fn errors(report: &ValidationReport) -> String {
    report
        .errors
        .iter()
        .map(ValidationIssue::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

fn default_true() -> bool {
    true
}
