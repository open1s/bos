//! Dependency and version resolution, plus the lockfile that makes an
//! installed set reproducible.
//!
//! Resolution is a depth-first walk over the dependency graph. Each extension
//! has exactly one selected version, the highest available that satisfies every
//! range pointing at it. A later range that excludes the selected version is a
//! conflict rather than a second copy of the same id, because two versions of
//! one extension registered in one host would share a storage namespace and a
//! tool namespace.

use std::collections::{BTreeMap, BTreeSet};

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use super::package::Package;
use super::types::{
    extension_api_version, host_version, ContributionKind, ExtensionError, ExtensionId,
    ExtensionSource, Requirement,
};

/// One resolved package in dependency-first order.
#[derive(Debug, Clone)]
pub(crate) struct Resolution {
    /// Packages to load, dependencies before dependents.
    pub(crate) packages: Vec<Package>,
    /// The lockfile describing exactly this set.
    pub(crate) lock: LockFile,
}

/// One package pinned by the lockfile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LockedPackage {
    /// Extension id.
    pub(crate) id: String,
    /// Exact version.
    pub(crate) version: String,
    /// Canonical package hash, empty for a built-in.
    pub(crate) hash: String,
    /// Where the package came from.
    pub(crate) source: ExtensionSource,
    /// Local directory or archive this package was installed from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) source_path: Option<String>,
    /// Git revision, when the source is a repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) revision: Option<String>,
    /// Installation timestamp in Unix seconds.
    #[serde(default)]
    pub(crate) installed_at: u64,
    /// Signature metadata shipped with the package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) signature: Option<crate::extension::signing::PackageSignature>,
    /// Publisher name from the signature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) publisher: Option<String>,
    /// Resolved dependency versions, id -> exact version.
    #[serde(default)]
    pub(crate) dependencies: BTreeMap<String, String>,
}

/// The dependency lockfile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LockFile {
    /// Format generation.
    #[serde(default = "lock_format")]
    pub(crate) format: u32,
    /// Host version that produced the resolution.
    pub(crate) host: String,
    /// Extension API generation the resolution was checked against.
    pub(crate) extension_api: String,
    /// Packages, sorted by id.
    #[serde(default)]
    pub(crate) packages: Vec<LockedPackage>,
}

fn lock_format() -> u32 {
    1
}

impl Default for LockFile {
    fn default() -> Self {
        Self {
            format: lock_format(),
            host: host_version().to_string(),
            extension_api: extension_api_version().to_string(),
            packages: Vec::new(),
        }
    }
}

impl LockFile {
    /// Build the lock from a resolved set.
    pub(crate) fn from_packages(packages: &[Package]) -> Self {
        let mut locked: Vec<LockedPackage> = packages
            .iter()
            .map(|package| LockedPackage {
                id: package.id().as_str().to_string(),
                version: package.manifest.extension.version.to_string(),
                hash: package.hash.clone(),
                source: package.source.clone(),
                source_path: package
                    .source_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
                revision: match &package.source {
                    crate::extension::types::ExtensionSource::Git { revision, .. } => {
                        Some(revision.clone())
                    }
                    _ => None,
                },
                installed_at: package.installed_at,
                signature: package.signature.clone(),
                publisher: package
                    .signature
                    .as_ref()
                    .map(|signature| signature.publisher.clone()),
                dependencies: package
                    .manifest
                    .dependencies
                    .keys()
                    .filter_map(|id| {
                        packages
                            .iter()
                            .find(|candidate| candidate.id() == id)
                            .map(|candidate| {
                                (
                                    id.as_str().to_string(),
                                    candidate.manifest.extension.version.to_string(),
                                )
                            })
                    })
                    .collect(),
            })
            .collect();
        locked.sort_by(|left, right| left.id.cmp(&right.id));
        Self {
            format: lock_format(),
            host: host_version().to_string(),
            extension_api: extension_api_version().to_string(),
            packages: locked,
        }
    }

    /// Serialize deterministically.
    pub(crate) fn to_toml(&self) -> Result<String, ExtensionError> {
        toml::to_string_pretty(self).map_err(|error| ExtensionError::Corrupt {
            path: std::path::PathBuf::from("lock.toml"),
            message: error.to_string(),
        })
    }

    /// Parse a lockfile.
    pub(crate) fn parse(text: &str) -> Result<Self, ExtensionError> {
        toml::from_str(text).map_err(|error| ExtensionError::Corrupt {
            path: std::path::PathBuf::from("lock.toml"),
            message: error.to_string(),
        })
    }

    /// The locked version of one extension.
    pub(crate) fn version_of(&self, id: &str) -> Option<&str> {
        self.packages
            .iter()
            .find(|package| package.id == id)
            .map(|package| package.version.as_str())
    }
}

/// Resolve a set of root extensions against the packages available to the
/// store. Roots are installed packages the host intends to load; their
/// transitive dependencies are added from the same pool.
pub(crate) fn resolve(
    available: &[Package],
    roots: &[ExtensionId],
) -> Result<Resolution, ExtensionError> {
    let mut pool: BTreeMap<String, BTreeMap<Version, &Package>> = BTreeMap::new();
    for package in available {
        pool.entry(package.id().as_str().to_string())
            .or_default()
            .insert(package.manifest.extension.version.clone(), package);
    }
    let mut selected: BTreeMap<String, &Package> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut visiting: Vec<String> = Vec::new();
    let mut stack: BTreeSet<String> = BTreeSet::new();
    for root in roots {
        visit(
            root.as_str(),
            &VersionReq::STAR,
            "<root>",
            &pool,
            &mut selected,
            &mut order,
            &mut visiting,
            &mut stack,
        )?;
    }
    let mut packages = Vec::with_capacity(order.len());
    for id in &order {
        packages.push(
            selected
                .get(id)
                .copied()
                .expect("every ordered id was selected")
                .clone(),
        );
    }
    let lock = LockFile::from_packages(&packages);
    Ok(Resolution { packages, lock })
}

#[allow(clippy::too_many_arguments)]
fn visit<'a>(
    id: &str,
    requirement: &VersionReq,
    required_by: &str,
    pool: &BTreeMap<String, BTreeMap<Version, &'a Package>>,
    selected: &mut BTreeMap<String, &'a Package>,
    order: &mut Vec<String>,
    visiting: &mut Vec<String>,
    stack: &mut BTreeSet<String>,
) -> Result<(), ExtensionError> {
    if let Some(existing) = selected.get(id) {
        let existing_version = &existing.manifest.extension.version;
        if requirement.matches(existing_version) {
            return Ok(());
        }
        return Err(ExtensionError::Dependency {
            extension: required_by.to_string(),
            message: format!(
                "requires {id} {requirement}, but {id} {existing_version} is already selected by another dependency"
            ),
        });
    }
    if let Some(position) = visiting.iter().position(|candidate| candidate == id) {
        let mut chain: Vec<String> = visiting[position..].to_vec();
        chain.push(id.to_string());
        return Err(ExtensionError::Cycle {
            chain: chain.join(" -> "),
        });
    }
    let Some(versions) = pool.get(id) else {
        return Err(ExtensionError::NoMatchingVersion {
            id: id.to_string(),
            requirement: requirement.to_string(),
        });
    };
    let chosen = versions
        .iter()
        .rev()
        .find(|(version, _)| requirement.matches(version))
        .map(|(_, package)| *package);
    let Some(package) = chosen else {
        return Err(ExtensionError::NoMatchingVersion {
            id: id.to_string(),
            requirement: requirement.to_string(),
        });
    };
    if !package.report.is_valid() {
        return Err(ExtensionError::Manifest {
            path: package.root.join(super::manifest::MANIFEST_FILE),
            message: package.error_message(),
        });
    }
    if !package.manifest.compatibility.bsh.matches(&host_version()) {
        return Err(ExtensionError::Incompatible {
            extension: id.to_string(),
            requirement: package.manifest.compatibility.bsh.to_string(),
            host: host_version().to_string(),
        });
    }
    visiting.push(id.to_string());
    stack.insert(id.to_string());
    let dependencies: Vec<(ExtensionId, Requirement)> = package
        .manifest
        .dependencies
        .iter()
        .map(|(id, requirement)| (id.clone(), requirement.clone()))
        .collect();
    for (dependency, requirement) in dependencies {
        visit(
            dependency.as_str(),
            &requirement.parsed,
            id,
            pool,
            selected,
            order,
            visiting,
            stack,
        )?;
    }
    stack.remove(id);
    visiting.pop();
    selected.insert(id.to_string(), package);
    order.push(id.to_string());
    Ok(())
}

/// A compact inventory used by `bsh ext info` and diagnostics.
#[derive(Debug, Clone)]
pub(crate) struct InventoryRow {
    /// Extension id.
    pub(crate) id: String,
    /// Exact version.
    pub(crate) version: String,
    /// Tool count.
    pub(crate) tools: usize,
    /// Contribution count, all kinds.
    pub(crate) contributions: usize,
}

impl InventoryRow {
    /// Build one row from a package.
    pub(crate) fn of(package: &Package) -> Self {
        let contributions = [
            ContributionKind::Tool,
            ContributionKind::Command,
            ContributionKind::Panel,
            ContributionKind::Prompt,
            ContributionKind::Context,
            ContributionKind::Workflow,
            ContributionKind::Theme,
        ]
        .into_iter()
        .map(|kind| package.manifest.contribution_ids(kind).len())
        .sum();
        Self {
            id: package.id().as_str().to_string(),
            version: package.manifest.extension.version.to_string(),
            tools: package.manifest.contributes.tools.len(),
            contributions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::manifest::Manifest;
    use crate::extension::types::{TrustLevel, ValidationReport};

    fn package(id: &str, version: &str, dependencies: &[(&str, &str)]) -> Package {
        let mut manifest = format!(
            r#"
schema_version = "1.0"

[extension]
id = "{id}"
name = "Test"
version = "{version}"
description = "Test package"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "declarative"

[contributes]
tools = ["echo"]
"#
        );
        if !dependencies.is_empty() {
            manifest.push_str("\n[dependencies]\n");
            for (dependency, requirement) in dependencies {
                manifest.push_str(&format!("\"{dependency}\" = \"{requirement}\"\n"));
            }
        }
        let parsed =
            Manifest::parse(&manifest, std::path::Path::new("extension.toml")).expect("manifest");
        let report = parsed.validate();
        assert!(report.is_valid(), "{report:?}");
        Package {
            root: std::path::PathBuf::from(format!("/tmp/{id}-{version}")),
            manifest: parsed,
            report: ValidationReport::default(),
            source: ExtensionSource::Installed,
            trust: TrustLevel::Local,
            hash: format!("hash-{id}-{version}"),
            installed_at: 0,
            signature: None,
            verification: crate::extension::signing::SignatureVerification::Missing,
            source_path: None,
        }
    }

    fn id(raw: &str) -> ExtensionId {
        ExtensionId::parse(raw).expect("id")
    }

    #[test]
    fn dependencies_are_ordered_before_dependents() {
        let a = package("com.example.a", "1.0.0", &[("com.example.c", "^1")]);
        let b = package("com.example.b", "1.0.0", &[("com.example.c", "^1")]);
        let c = package("com.example.c", "1.2.0", &[]);
        let resolution =
            resolve(&[a, b, c], &[id("com.example.a"), id("com.example.b")]).expect("resolve");
        let ids: Vec<&str> = resolution
            .packages
            .iter()
            .map(|package| package.id().as_str())
            .collect();
        assert_eq!(ids, vec!["com.example.c", "com.example.a", "com.example.b"]);
        assert_eq!(resolution.lock.packages.len(), 3);
    }

    #[test]
    fn the_highest_matching_version_is_selected() {
        let roots = [package(
            "com.example.a",
            "1.0.0",
            &[("com.example.b", "^1")],
        )];
        let available = [
            roots[0].clone(),
            package("com.example.b", "1.0.0", &[]),
            package("com.example.b", "1.9.0", &[]),
            package("com.example.b", "2.0.0", &[]),
        ];
        let resolution = resolve(&available, &[id("com.example.a")]).expect("resolve");
        assert_eq!(
            resolution
                .packages
                .iter()
                .find(|package| package.id().as_str() == "com.example.b")
                .expect("b")
                .manifest
                .extension
                .version
                .to_string(),
            "1.9.0"
        );
    }

    #[test]
    fn a_missing_dependency_is_named() {
        let available = [package(
            "com.example.a",
            "1.0.0",
            &[("com.example.missing", "^1")],
        )];
        let error = resolve(&available, &[id("com.example.a")]).expect_err("missing");
        assert!(matches!(error, ExtensionError::NoMatchingVersion { .. }));
        assert!(error.to_string().contains("com.example.missing"));
    }

    #[test]
    fn a_cycle_is_reported_with_its_chain() {
        let available = [
            package("com.example.a", "1.0.0", &[("com.example.b", "^1")]),
            package("com.example.b", "1.0.0", &[("com.example.a", "^1")]),
        ];
        let error = resolve(&available, &[id("com.example.a")]).expect_err("cycle");
        assert!(matches!(error, ExtensionError::Cycle { .. }));
        assert!(error.to_string().contains("com.example.a"));
    }

    #[test]
    fn conflicting_ranges_on_one_id_are_refused() {
        let available = [
            package("com.example.a", "1.0.0", &[("com.example.c", "^1")]),
            package("com.example.b", "1.0.0", &[("com.example.c", "^2")]),
            package("com.example.c", "1.0.0", &[]),
            package("com.example.c", "2.0.0", &[]),
        ];
        let error =
            resolve(&available, &[id("com.example.a"), id("com.example.b")]).expect_err("conflict");
        assert!(
            matches!(error, ExtensionError::Dependency { .. }),
            "{error}"
        );
    }

    #[test]
    fn a_lockfile_round_trips_and_is_sorted() {
        let a = package("com.example.a", "1.0.0", &[]);
        let b = package("com.example.b", "2.0.0", &[]);
        let resolution =
            resolve(&[b, a], &[id("com.example.b"), id("com.example.a")]).expect("resolve");
        let text = resolution.lock.to_toml().expect("lock");
        let parsed = LockFile::parse(&text).expect("parse");
        assert_eq!(parsed, resolution.lock);
        let ids: Vec<&str> = parsed
            .packages
            .iter()
            .map(|package| package.id.as_str())
            .collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted);
    }
}
