//! Resolving slots against a catalogue of providers.
//!
//! Resolution is exact: a slot names a surface contract and a version range,
//! and the catalogue either has exactly one matching provider or the plan
//! says why not. There is no directory-order or hash-order tie-break, and no
//! implicit fallback to a nearby contract.

use semver::VersionReq;
use serde::{Deserialize, Serialize};

use super::contract::{Profile, SurfaceContract};
use super::node::SurfaceNode;
use super::{SurfaceError, SurfaceId, SurfaceLimits};

/// A request for one surface provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceRequirement {
    /// The provider-qualified surface.
    pub surface: SurfaceId,
    /// Accepted contract versions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// A profile the caller needs, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<Profile>,
}

impl SurfaceRequirement {
    /// Require a surface at any version.
    pub fn new(surface: SurfaceId) -> Self {
        Self {
            surface,
            version: None,
            profile: None,
        }
    }

    /// Require a version range.
    pub fn with_version(mut self, range: impl Into<String>) -> Result<Self, SurfaceError> {
        let range = range.into();
        VersionReq::parse(&range).map_err(|error| {
            SurfaceError::Invalid(format!(
                "version range {range:?} is not valid semver: {error}"
            ))
        })?;
        self.version = Some(range);
        Ok(self)
    }

    /// Require a rendering profile.
    #[must_use]
    pub fn with_profile(mut self, profile: Profile) -> Self {
        self.profile = Some(profile);
        self
    }

    fn parsed_range(&self) -> Result<Option<VersionReq>, SurfaceError> {
        match &self.version {
            None => Ok(None),
            Some(range) => VersionReq::parse(range).map(Some).map_err(|error| {
                SurfaceError::Invalid(format!(
                    "version range {range:?} is not valid semver: {error}"
                ))
            }),
        }
    }
}

/// The providers the host knows about.
#[derive(Debug, Clone, Default)]
pub struct SurfaceCatalog {
    contracts: Vec<SurfaceContract>,
}

impl SurfaceCatalog {
    /// An empty catalogue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one provider contract.
    pub fn register(&mut self, contract: SurfaceContract) -> Result<(), SurfaceError> {
        contract.ensure_valid()?;
        self.contracts.push(contract);
        Ok(())
    }

    /// The registered contracts.
    pub fn contracts(&self) -> &[SurfaceContract] {
        &self.contracts
    }

    /// Resolve exactly one provider for a requirement.
    pub fn resolve(
        &self,
        requirement: &SurfaceRequirement,
    ) -> Result<&SurfaceContract, SurfaceError> {
        let range = requirement.parsed_range()?;
        let matching: Vec<&SurfaceContract> = self
            .contracts
            .iter()
            .filter(|contract| contract.surface == requirement.surface)
            .filter(|contract| {
                range
                    .as_ref()
                    .is_none_or(|range| range.matches(&contract.contract_version))
            })
            .filter(|contract| {
                requirement
                    .profile
                    .is_none_or(|profile| contract.supports(profile))
            })
            .collect();
        match matching.as_slice() {
            [] => Err(SurfaceError::NoProvider(requirement.surface.to_string())),
            [one] => Ok(one),
            many => {
                let mut candidates: Vec<String> = many
                    .iter()
                    .map(|contract| {
                        format!(
                            "{}@{} ({:?})",
                            contract.surface, contract.contract_version, contract.isolation
                        )
                    })
                    .collect();
                candidates.sort();
                Err(SurfaceError::AmbiguousProvider {
                    surface: requirement.surface.to_string(),
                    candidates,
                })
            }
        }
    }

    /// The profiles every provider of this surface supports, for diagnostics.
    pub fn profiles_of(&self, surface: &SurfaceId) -> Vec<Profile> {
        let mut profiles: Vec<Profile> = Vec::new();
        for contract in self
            .contracts
            .iter()
            .filter(|contract| &contract.surface == surface)
        {
            for profile in &contract.profiles {
                if !profiles.contains(profile) {
                    profiles.push(*profile);
                }
            }
        }
        profiles.sort();
        profiles
    }
}

/// One resolved slot.
#[derive(Debug, Clone, PartialEq)]
pub struct SlotMount {
    /// The path from the root document to the slot node.
    pub path: Vec<String>,
    /// What the slot asked for.
    pub requirement: SurfaceRequirement,
    /// The provider that answered.
    pub contract: SurfaceContract,
}

/// One slot the catalogue could not answer.
#[derive(Debug, Clone, PartialEq)]
pub struct UnresolvedSlot {
    /// The path from the root document to the slot node.
    pub path: Vec<String>,
    /// The requested surface.
    pub surface: SurfaceId,
    /// Whether the slot is required.
    pub required: bool,
    /// The resolution failure, for the host to explain.
    pub reason: String,
}

/// The slots of one document, resolved.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CompositionPlan {
    /// Slots that resolved to exactly one provider.
    pub mounts: Vec<SlotMount>,
    /// Required or optional slots with no provider.
    pub unresolved: Vec<UnresolvedSlot>,
}

/// Resolve every slot in a document.
///
/// The walk covers the document's own slots; a mounted child surface has its
/// own document and is planned when its frame arrives. Cycles are detected
/// against the chain of surfaces entered so far, and the depth and fan-out
/// ceilings are enforced as the walk descends.
pub fn plan_composition(
    root: &SurfaceNode,
    catalog: &SurfaceCatalog,
    limits: &SurfaceLimits,
) -> Result<CompositionPlan, SurfaceError> {
    let mut plan = CompositionPlan::default();
    let mut ancestors: Vec<SurfaceId> = Vec::new();
    let mut path: Vec<String> = Vec::new();
    walk(root, catalog, limits, &mut ancestors, &mut path, &mut plan)?;
    Ok(plan)
}

fn walk(
    node: &SurfaceNode,
    catalog: &SurfaceCatalog,
    limits: &SurfaceLimits,
    ancestors: &mut Vec<SurfaceId>,
    path: &mut Vec<String>,
    plan: &mut CompositionPlan,
) -> Result<(), SurfaceError> {
    if let Some(children) = node.container_children() {
        let slots_here = children.iter().filter(|child| child.is_slot()).count();
        if slots_here > limits.max_slot_fanout {
            return Err(SurfaceError::Budget(format!(
                "a node holds {slots_here} slots; the ceiling is {}",
                limits.max_slot_fanout
            )));
        }
        for child in children {
            walk(child, catalog, limits, ancestors, path, plan)?;
        }
    }
    if let SurfaceNode::Tabs { tabs, .. } = node {
        for tab in tabs {
            walk(&tab.content, catalog, limits, ancestors, path, plan)?;
        }
    }
    if let SurfaceNode::Slot {
        id,
        surface,
        version,
        required,
        fallback,
        ..
    } = node
    {
        let surface_id = SurfaceId::parse(surface.clone())?;
        let mut requirement = SurfaceRequirement::new(surface_id.clone());
        if let Some(version) = version {
            requirement = requirement.with_version(version.clone())?;
        }
        let leaf = id
            .clone()
            .unwrap_or_else(|| format!("slot-{}", plan.mounts.len() + plan.unresolved.len()));
        path.push(leaf);
        if ancestors.contains(&surface_id) {
            let chain = ancestors
                .iter()
                .map(ToString::to_string)
                .chain(std::iter::once(surface_id.to_string()))
                .collect::<Vec<_>>()
                .join(" -> ");
            return Err(SurfaceError::Cycle(chain));
        }
        if ancestors.len() >= limits.max_slot_depth {
            return Err(SurfaceError::Budget(format!(
                "composition nests deeper than {} surfaces at {}",
                limits.max_slot_depth, surface_id
            )));
        }
        match catalog.resolve(&requirement) {
            Ok(contract) => {
                plan.mounts.push(SlotMount {
                    path: path.clone(),
                    requirement: requirement.clone(),
                    contract: contract.clone(),
                });
                ancestors.push(surface_id);
                if let Some(fallback) = fallback {
                    walk(fallback, catalog, limits, ancestors, path, plan)?;
                }
                ancestors.pop();
            }
            Err(error @ SurfaceError::AmbiguousProvider { .. }) => return Err(error),
            Err(error) => plan.unresolved.push(UnresolvedSlot {
                path: path.clone(),
                surface: surface_id,
                required: *required,
                reason: error.to_string(),
            }),
        }
        path.pop();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::contract::{ActionSpec, Profile, SurfaceContract};
    use crate::surface::node::{Sizing, SurfaceNode};
    use crate::surface::{Confirmation, SurfaceLimits};
    use crate::Risk;
    use semver::Version;

    fn contract(name: &str, version: (u64, u64, u64)) -> SurfaceContract {
        SurfaceContract::new(
            SurfaceId::parse(format!("acme.{name}/panel")).expect("id"),
            Version::new(version.0, version.1, version.2),
        )
        .with_profile(Profile::Text)
        .with_action(ActionSpec {
            id: "open".to_string(),
            label: "Open".to_string(),
            capability: format!("acme.{name}/open"),
            risk: Risk::Low,
            confirmation: Confirmation::Inherit,
        })
    }

    fn slot(surface: &str, fallback: Option<SurfaceNode>) -> SurfaceNode {
        SurfaceNode::Slot {
            id: None,
            surface: surface.to_string(),
            version: None,
            required: fallback.is_none(),
            props: serde_json::Value::Null,
            sizing: Sizing::default(),
            fallback: fallback.map(Box::new),
        }
    }

    #[test]
    fn one_provider_resolves_exactly() {
        let mut catalog = SurfaceCatalog::new();
        catalog
            .register(contract("git", (1, 2, 0)))
            .expect("register");
        let requirement = SurfaceRequirement::new(SurfaceId::parse("acme.git/panel").expect("id"));
        let resolved = catalog.resolve(&requirement).expect("resolve");
        assert_eq!(resolved.contract_version, Version::new(1, 2, 0));
    }

    #[test]
    fn two_providers_are_an_ambiguity_not_a_coin_flip() {
        let mut catalog = SurfaceCatalog::new();
        catalog
            .register(contract("git", (1, 2, 0)))
            .expect("register");
        catalog
            .register(contract("git", (1, 3, 0)))
            .expect("register");
        let requirement = SurfaceRequirement::new(SurfaceId::parse("acme.git/panel").expect("id"));
        match catalog.resolve(&requirement) {
            Err(SurfaceError::AmbiguousProvider { candidates, .. }) => {
                assert_eq!(candidates.len(), 2);
            }
            other => panic!("expected ambiguity, got {other:?}"),
        }
    }

    #[test]
    fn a_version_range_that_matches_nothing_is_no_provider() {
        let mut catalog = SurfaceCatalog::new();
        catalog
            .register(contract("git", (2, 0, 0)))
            .expect("register");
        let requirement = SurfaceRequirement::new(SurfaceId::parse("acme.git/panel").expect("id"))
            .with_version("^1")
            .expect("range");
        assert!(matches!(
            catalog.resolve(&requirement),
            Err(SurfaceError::NoProvider(_))
        ));
    }

    #[test]
    fn a_missing_provider_is_unresolved_not_a_failure() {
        let root = SurfaceNode::Column {
            id: None,
            children: vec![slot("acme.git/panel", None)],
        };
        let plan = plan_composition(&root, &SurfaceCatalog::new(), &SurfaceLimits::default())
            .expect("plan");
        assert_eq!(plan.mounts.len(), 0);
        assert_eq!(plan.unresolved.len(), 1);
        assert!(plan.unresolved[0].required);
    }

    #[test]
    fn a_cycle_is_refused() {
        let mut catalog = SurfaceCatalog::new();
        catalog
            .register(contract("a", (1, 0, 0)))
            .expect("register");
        catalog
            .register(contract("b", (1, 0, 0)))
            .expect("register");
        let root = SurfaceNode::Column {
            id: None,
            children: vec![slot(
                "acme.a/panel",
                Some(slot("acme.b/panel", Some(slot("acme.a/panel", None)))),
            )],
        };
        assert!(matches!(
            plan_composition(&root, &catalog, &SurfaceLimits::default()),
            Err(SurfaceError::Cycle(_))
        ));
    }

    #[test]
    fn slot_fanout_is_bounded() {
        let limits = SurfaceLimits {
            max_slot_fanout: 1,
            ..SurfaceLimits::default()
        };
        let root = SurfaceNode::Column {
            id: None,
            children: vec![slot("acme.a/panel", None), slot("acme.b/panel", None)],
        };
        assert!(matches!(
            plan_composition(&root, &SurfaceCatalog::new(), &limits),
            Err(SurfaceError::Budget(_))
        ));
    }

    #[test]
    fn composition_depth_is_bounded() {
        let mut catalog = SurfaceCatalog::new();
        for name in ["a", "b", "c", "d", "e"] {
            catalog
                .register(contract(name, (1, 0, 0)))
                .expect("register");
        }
        let mut node = slot("acme.e/panel", None);
        for name in ["d", "c", "b", "a"] {
            node = slot(&format!("acme.{name}/panel"), Some(node));
        }
        let root = SurfaceNode::Column {
            id: None,
            children: vec![node],
        };
        let limits = SurfaceLimits {
            max_slot_depth: 2,
            ..SurfaceLimits::default()
        };
        assert!(matches!(
            plan_composition(&root, &catalog, &limits),
            Err(SurfaceError::Budget(_))
        ));
    }
}
