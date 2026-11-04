//! Composition profiles, groups, and patches.
//!
//! A profile is the host-level description of what to mount. Packages describe
//! what they can contribute; profiles select, group, configure, and patch the
//! live set. The planner is pure: it validates the whole plan before any effect
//! runs and returns a deterministic activation order.
//!
//! The file format is intentionally close to Cordis's loader tree: entries have
//! ids, groups, config, enabled flags, isolate maps, and dependencies. A
//! cordis.patch.yml row can be lowered into the same entry shape by the Cordis
//! runtime adapter.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use serde::Deserialize;

/// One profile file.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// Schema generation.
    #[serde(default)]
    pub schema_version: String,
    /// Profile metadata.
    #[serde(default)]
    pub profile: ProfileMeta,
    /// Top-level entries.
    #[serde(default)]
    pub entry: Vec<PlanEntry>,
    /// Ordered patches.
    #[serde(default)]
    pub patch: Vec<ProfilePatch>,
}

/// Profile identity and inheritance.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ProfileMeta {
    /// Profile name.
    #[serde(default)]
    pub name: String,
    /// Optional parent profile name. Loading the parent is a host concern.
    #[serde(default)]
    pub extends: Option<String>,
}

/// One entry in a profile.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PlanEntry {
    /// Stable id. A group's children may be addressed as group:child.
    pub id: String,
    /// Source: builtin:name, package:id, group, or cordis:package#row.
    #[serde(default)]
    pub source: String,
    /// Whether the entry activates.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Whether this entry is a group.
    #[serde(default)]
    pub group: bool,
    /// Child ids when group is true.
    #[serde(default)]
    pub entries: Vec<String>,
    /// Package/entry dependencies by id.
    #[serde(default)]
    pub dependencies: Vec<String>,
    /// Contribution ids owned by this entry, for conflict checks.
    #[serde(default)]
    pub contributes: Vec<String>,
    /// Component/package configuration.
    #[serde(default)]
    pub config: Option<serde_json::Value>,
    /// Per-service isolation realm names.
    #[serde(default)]
    pub isolate: BTreeMap<String, String>,
}

/// A patch applies after group expansion and before ordering.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ProfilePatch {
    /// Target entry id.
    pub id: String,
    /// Replacement config.
    #[serde(default)]
    pub config: Option<serde_json::Value>,
    /// Replacement enabled flag.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Replacement dependency list.
    #[serde(default)]
    pub dependencies: Option<Vec<String>>,
}

/// The validated activation plan.
#[derive(Debug, Clone, Default)]
pub struct ActivationPlan {
    /// Enabled entries in dependency-first order.
    pub order: Vec<String>,
    /// Every expanded entry, including disabled rows.
    pub entries: BTreeMap<String, PlanEntry>,
}

/// Why a profile cannot be activated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// Two entries use the same id.
    DuplicateId(String),
    /// A group lists an unknown child.
    UnknownEntry(String),
    /// A group cycle exists.
    GroupCycle(String),
    /// A dependency is not in the expanded plan.
    MissingDependency { entry: String, dependency: String },
    /// An enabled entry depends on an entry the profile disabled.
    DisabledDependency { entry: String, dependency: String },
    /// A dependency cycle exists.
    DependencyCycle(Vec<String>),
    /// Two enabled entries own the same contribution id.
    DuplicateContribution {
        contribution: String,
        first: String,
        second: String,
    },
    /// A patch targets an unknown entry.
    UnknownPatch(String),
    /// The file could not be read.
    Io(String),
    /// The TOML was invalid.
    Toml(String),
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateId(id) => write!(f, "duplicate profile entry {id}"),
            Self::UnknownEntry(id) => write!(f, "profile references unknown entry {id}"),
            Self::GroupCycle(id) => write!(f, "profile group cycle at {id}"),
            Self::MissingDependency { entry, dependency } => {
                write!(f, "{entry} depends on missing entry {dependency}")
            }
            Self::DisabledDependency { entry, dependency } => {
                write!(
                    f,
                    "{entry} is enabled but depends on disabled entry {dependency}"
                )
            }
            Self::DependencyCycle(members) => {
                write!(f, "profile dependency cycle: {}", members.join(" -> "))
            }
            Self::DuplicateContribution {
                contribution,
                first,
                second,
            } => write!(
                f,
                "contribution {contribution} is owned by both {first} and {second}"
            ),
            Self::UnknownPatch(id) => write!(f, "patch targets unknown entry {id}"),
            Self::Io(message) => write!(f, "profile read failed: {message}"),
            Self::Toml(message) => write!(f, "profile parse failed: {message}"),
        }
    }
}

impl std::error::Error for PlanError {}

/// The pure composition planner.
#[derive(Debug, Clone, Copy, Default)]
pub struct CompositionPlanner;

impl CompositionPlanner {
    /// Read and plan a profile file.
    pub fn load(path: &Path) -> Result<ActivationPlan, PlanError> {
        let text =
            std::fs::read_to_string(path).map_err(|error| PlanError::Io(error.to_string()))?;
        let profile: Profile =
            toml::from_str(&text).map_err(|error| PlanError::Toml(error.to_string()))?;
        Self::plan(&profile)
    }

    /// Expand groups, apply patches, validate, and order.
    pub fn plan(profile: &Profile) -> Result<ActivationPlan, PlanError> {
        let mut raw = BTreeMap::new();
        for entry in &profile.entry {
            if raw.insert(entry.id.clone(), entry.clone()).is_some() {
                return Err(PlanError::DuplicateId(entry.id.clone()));
            }
        }

        let child_ids: BTreeSet<String> = profile
            .entry
            .iter()
            .flat_map(|entry| entry.entries.iter().cloned())
            .collect();
        let mut entries = BTreeMap::new();
        let mut visiting = BTreeSet::new();
        let mut expanded = BTreeSet::new();
        for entry in &profile.entry {
            // Group children are expanded through their group, not again as
            // top-level rows.
            if child_ids.contains(&entry.id) {
                continue;
            }
            expand_entry(entry, &raw, &mut entries, &mut visiting, &mut expanded, 0)?;
        }

        for patch in &profile.patch {
            let Some(entry) = entries.get_mut(&patch.id) else {
                return Err(PlanError::UnknownPatch(patch.id.clone()));
            };
            if let Some(config) = &patch.config {
                entry.config = Some(config.clone());
            }
            if let Some(enabled) = patch.enabled {
                entry.enabled = enabled;
            }
            if let Some(dependencies) = &patch.dependencies {
                entry.dependencies = dependencies.clone();
            }
        }

        let mut contributions = BTreeMap::<String, String>::new();
        for entry in entries.values().filter(|entry| entry.enabled) {
            for contribution in &entry.contributes {
                if let Some(first) = contributions.insert(contribution.clone(), entry.id.clone()) {
                    return Err(PlanError::DuplicateContribution {
                        contribution: contribution.clone(),
                        first,
                        second: entry.id.clone(),
                    });
                }
            }
        }

        let order = order_entries(&entries)?;
        Ok(ActivationPlan { order, entries })
    }
}

fn default_true() -> bool {
    true
}

fn expand_entry(
    entry: &PlanEntry,
    raw: &BTreeMap<String, PlanEntry>,
    entries: &mut BTreeMap<String, PlanEntry>,
    visiting: &mut BTreeSet<String>,
    expanded: &mut BTreeSet<String>,
    depth: usize,
) -> Result<(), PlanError> {
    if entries.contains_key(&entry.id) {
        return Err(PlanError::DuplicateId(entry.id.clone()));
    }
    if depth > 32 {
        return Err(PlanError::GroupCycle(entry.id.clone()));
    }

    if !entry.group {
        entries.insert(entry.id.clone(), entry.clone());
        expanded.insert(entry.id.clone());
        return Ok(());
    }

    if !visiting.insert(entry.id.clone()) {
        return Err(PlanError::GroupCycle(entry.id.clone()));
    }
    // A group itself is not an activation target; only its children are.
    for child_id in &entry.entries {
        let child = raw
            .get(child_id)
            .ok_or_else(|| PlanError::UnknownEntry(child_id.clone()))?;
        if !entry.enabled {
            // A disabled group keeps its children in the plan, disabled.
            let mut child = child.clone();
            child.enabled = false;
            expand_entry(&child, raw, entries, visiting, expanded, depth + 1)?;
        } else {
            expand_entry(child, raw, entries, visiting, expanded, depth + 1)?;
        }
    }
    visiting.remove(&entry.id);
    Ok(())
}

fn order_entries(entries: &BTreeMap<String, PlanEntry>) -> Result<Vec<String>, PlanError> {
    let enabled: BTreeSet<&String> = entries
        .iter()
        .filter_map(|(id, entry)| entry.enabled.then_some(id))
        .collect();

    let mut indegree = BTreeMap::<String, usize>::new();
    let mut dependents = BTreeMap::<String, BTreeSet<String>>::new();
    for id in &enabled {
        indegree.insert((*id).clone(), 0);
    }
    for id in &enabled {
        let entry = &entries[*id];
        for dependency in &entry.dependencies {
            if !entries.contains_key(dependency) {
                return Err(PlanError::MissingDependency {
                    entry: (*id).clone(),
                    dependency: dependency.clone(),
                });
            }
            if !enabled.contains(dependency) {
                // Treating a disabled dependency as satisfied activated a
                // package whose required service was never mounted.
                return Err(PlanError::DisabledDependency {
                    entry: (*id).clone(),
                    dependency: dependency.clone(),
                });
            }
            *indegree.get_mut(*id).expect("inserted above") += 1;
            dependents
                .entry(dependency.clone())
                .or_default()
                .insert((*id).clone());
        }
    }

    let mut ready: BTreeSet<String> = indegree
        .iter()
        .filter_map(|(id, degree)| (*degree == 0).then_some(id.clone()))
        .collect();
    let mut order = Vec::with_capacity(enabled.len());
    while let Some(id) = ready.pop_first() {
        order.push(id.clone());
        if let Some(children) = dependents.get(&id) {
            for child in children {
                let degree = indegree.get_mut(child).expect("known dependent");
                *degree -= 1;
                if *degree == 0 {
                    ready.insert(child.clone());
                }
            }
        }
    }
    if order.len() != enabled.len() {
        let members = indegree
            .into_iter()
            .filter_map(|(id, degree)| (degree > 0).then_some(id))
            .collect();
        return Err(PlanError::DependencyCycle(members));
    }
    Ok(order)
}

/// Lower a Cordis bundle patch file into profile entries.
pub fn load_cordis_patch(path: &Path, package: &str) -> Result<Vec<PlanEntry>, PlanError> {
    let text = std::fs::read_to_string(path).map_err(|error| PlanError::Io(error.to_string()))?;
    parse_cordis_patch(&text, package)
}

/// Lower Cordis patch YAML text into profile entries.
///
/// The accepted shape is the one DSH ships: a sequence of operations, or one
/// mapping with an insert sequence. Each inserted row becomes a PlanEntry whose
/// source is `cordis:<package>#<name>`; unknown non-insert rows are ignored here
/// and refused by the caller's plan validation if they are referenced.
pub fn parse_cordis_patch(text: &str, package: &str) -> Result<Vec<PlanEntry>, PlanError> {
    let root: serde_yaml::Value =
        serde_yaml::from_str(text).map_err(|error| PlanError::Toml(error.to_string()))?;
    let mut rows = Vec::new();
    match &root {
        serde_yaml::Value::Sequence(items) => {
            for item in items {
                collect_inserts(item, &mut rows);
            }
        }
        serde_yaml::Value::Mapping(_) => collect_inserts(&root, &mut rows),
        _ => {}
    }

    let mut entries = Vec::new();
    for row in rows {
        let name = map_get(row, "name")
            .and_then(serde_yaml::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let id = map_get(row, "id")
            .and_then(serde_yaml::Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .or_else(|| (!name.is_empty()).then(|| name.clone()))
            .ok_or_else(|| PlanError::Toml("cordis row has no id or name".to_string()))?;
        let enabled = !map_get(row, "disabled")
            .and_then(serde_yaml::Value::as_bool)
            .unwrap_or(false);
        let group = map_get(row, "group")
            .and_then(serde_yaml::Value::as_bool)
            .unwrap_or(false);
        let mut children = Vec::new();
        if let Some(entries) = map_get(row, "entries").and_then(serde_yaml::Value::as_sequence) {
            children.extend(
                entries
                    .iter()
                    .filter_map(serde_yaml::Value::as_str)
                    .map(str::to_owned),
            );
        }
        let mut isolate = BTreeMap::new();
        if let Some(mapping) = map_get(row, "isolate").and_then(serde_yaml::Value::as_mapping) {
            for (key, value) in mapping {
                if let (Some(key), Some(value)) = (key.as_str(), value.as_str()) {
                    isolate.insert(key.to_owned(), value.to_owned());
                }
            }
        }
        let config = map_get(row, "config").and_then(|value| serde_json::to_value(value).ok());
        entries.push(PlanEntry {
            id,
            source: format!("cordis:{package}#{name}"),
            enabled,
            group,
            entries: children,
            dependencies: Vec::new(),
            contributes: Vec::new(),
            config,
            isolate,
        });
    }
    Ok(entries)
}

fn collect_inserts<'a>(item: &'a serde_yaml::Value, rows: &mut Vec<&'a serde_yaml::Value>) {
    match map_get(item, "insert") {
        Some(serde_yaml::Value::Sequence(inserts)) => rows.extend(inserts),
        Some(serde_yaml::Value::Mapping(_)) => rows.push(item),
        _ => {
            if map_get(item, "id").is_some() || map_get(item, "name").is_some() {
                rows.push(item);
            }
        }
    }
}

fn map_get<'a>(value: &'a serde_yaml::Value, key: &str) -> Option<&'a serde_yaml::Value> {
    match value {
        serde_yaml::Value::Mapping(mapping) => {
            mapping.get(serde_yaml::Value::String(key.to_owned()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<ActivationPlan, PlanError> {
        let profile: Profile = toml::from_str(text).expect("profile toml");
        CompositionPlanner::plan(&profile)
    }

    #[test]
    #[ignore = "release benchmark; run with --release --ignored"]
    fn gold_standard_release_plan_budget() {
        let mut profile = Profile::default();
        for index in 0..1_000 {
            let id = format!("e{index}");
            let dependencies = if index > 0 {
                vec![format!("e{}", index - 1)]
            } else {
                Vec::new()
            };
            profile.entry.push(PlanEntry {
                id,
                source: "builtin:test".to_string(),
                enabled: true,
                group: false,
                entries: Vec::new(),
                dependencies,
                contributes: Vec::new(),
                config: None,
                isolate: BTreeMap::new(),
            });
        }
        let start = std::time::Instant::now();
        let plan = CompositionPlanner::plan(&profile).expect("plan");
        let elapsed = start.elapsed();
        assert_eq!(plan.order.len(), 1_000);
        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "1k-fiber plan took {elapsed:?} in release"
        );
    }

    #[test]
    fn cordis_patch_rows_lower_to_profile_entries() {
        let entries = parse_cordis_patch(
            r#"
            - insert:
                - id: dsh-echo
                  name: '@local/dsh-echo'
                  config:
                    greeting: hi
                  disabled: false
            "#,
            "com.example.pkg",
        )
        .expect("patch");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "dsh-echo");
        assert_eq!(entries[0].source, "cordis:com.example.pkg#@local/dsh-echo");
        assert!(entries[0].enabled);
        assert_eq!(entries[0].config.as_ref().unwrap()["greeting"], "hi");
    }

    #[test]
    fn groups_expand_and_order_providers_first() {
        let plan = parse(
            r#"
            [[entry]]
            id = "tools"
            group = true
            entries = ["tools.git", "tools.search"]

            [[entry]]
            id = "tools.git"
            source = "builtin:git"

            [[entry]]
            id = "tools.search"
            source = "package:com.example.search"
            dependencies = ["tools.git"]
            "#,
        )
        .expect("plan");
        assert_eq!(plan.order, vec!["tools.git", "tools.search"]);
        assert!(plan.entries.contains_key("tools.git"));
    }

    #[test]
    fn disabled_group_disables_children() {
        let plan = parse(
            r#"
            [[entry]]
            id = "tools"
            group = true
            enabled = false
            entries = ["tools.git"]

            [[entry]]
            id = "tools.git"
            source = "builtin:git"
            "#,
        )
        .expect("plan");
        assert!(plan.order.is_empty());
        assert!(!plan.entries["tools.git"].enabled);
    }

    #[test]
    fn patch_changes_config_and_enabled() {
        let plan = parse(
            r#"
            [[entry]]
            id = "tools.git"
            source = "builtin:git"
            enabled = false

            [[patch]]
            id = "tools.git"
            enabled = true
            config = { status = true }
            "#,
        )
        .expect("plan");
        assert_eq!(plan.order, vec!["tools.git"]);
        assert!(plan.entries["tools.git"].enabled);
        assert!(plan.entries["tools.git"].config.is_some());
    }

    #[test]
    fn duplicate_contributions_are_rejected() {
        let error = parse(
            r#"
            [[entry]]
            id = "a"
            contributes = ["tool:git"]

            [[entry]]
            id = "b"
            contributes = ["tool:git"]
            "#,
        )
        .expect_err("duplicate");
        assert!(matches!(error, PlanError::DuplicateContribution { .. }));
    }

    #[test]
    fn missing_dependency_and_cycles_are_rejected() {
        let missing = parse(
            r#"
            [[entry]]
            id = "a"
            dependencies = ["missing"]
            "#,
        )
        .expect_err("missing");
        assert!(matches!(missing, PlanError::MissingDependency { .. }));

        let cycle = parse(
            r#"
            [[entry]]
            id = "a"
            dependencies = ["b"]

            [[entry]]
            id = "b"
            dependencies = ["a"]
            "#,
        )
        .expect_err("cycle");
        assert!(matches!(cycle, PlanError::DependencyCycle(_)));
    }

    #[test]
    fn an_enabled_entry_cannot_depend_on_a_disabled_entry() {
        let error = parse(
            r#"
            [[entry]]
            id = "a"
            dependencies = ["b"]

            [[entry]]
            id = "b"
            enabled = false
            "#,
        )
        .expect_err("a disabled dependency is not satisfied");
        assert!(
            matches!(
                error,
                PlanError::DisabledDependency { ref entry, ref dependency }
                    if entry == "a" && dependency == "b"
            ),
            "{error}"
        );
    }
}
