//! Manifest 2.0.
//!
//! The v2 envelope is host-neutral: runtime kinds, capability names and
//! contribution kinds are strings owned by the host. The envelope parser
//! rejects unknown top-level fields and leaves opaque payloads to the host.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::manifest::{
    ConfigurationSpec, Identity, Manifest, ResourcesSpec, ServicesSpec, MANIFEST_FILE,
    MAX_MANIFEST_BYTES,
};
use crate::types::{ContributionId, ExtensionError, ExtensionId, Requirement};

/// The only schema generation this parser accepts.
pub const MANIFEST_V2_SCHEMA_VERSION: &str = "2.0";

/// The host-neutral compatibility block.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompatibilityV2 {
    /// Host versions this package supports.
    pub host: Requirement,
    /// Extension API generations this package uses.
    pub extension_api: Requirement,
}

/// The runtime envelope. The kind and config schema are host-registered.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSpecV2 {
    /// Host-registered runtime kind name.
    pub kind: String,
    /// Opaque runtime configuration validated by the runtime kind's schema.
    #[serde(default = "empty_table")]
    pub config: toml::Value,
}

fn empty_table() -> toml::Value {
    toml::Value::Table(toml::map::Map::new())
}

/// The required-only capability list.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilitiesV2 {
    /// Host-registered capability names.
    #[serde(default)]
    pub required: Vec<String>,
}

/// One generic contribution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContributionSpecV2 {
    /// Host-registered contribution kind.
    pub kind: String,
    /// Contribution id inside the extension.
    pub id: ContributionId,
    /// Opaque payload validated by the kind's schema.
    #[serde(default = "empty_table")]
    pub payload: toml::Value,
}

/// A complete manifest 2.0 document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestV2 {
    /// Must be "2.0".
    pub schema_version: String,
    /// Identity block.
    pub extension: Identity,
    /// Compatibility block.
    pub compatibility: CompatibilityV2,
    /// Runtime block.
    pub runtime: RuntimeSpecV2,
    /// Required capabilities.
    #[serde(default)]
    pub capabilities: CapabilitiesV2,
    /// Generic contributions.
    #[serde(default, rename = "contribution")]
    pub contributions: Vec<ContributionSpecV2>,
    /// Configuration block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<ConfigurationSpec>,
    /// Resource block.
    #[serde(default)]
    pub resources: ResourcesSpec,
    /// Dependency map.
    #[serde(default)]
    pub dependencies: BTreeMap<ExtensionId, Requirement>,
    /// Service contracts.
    #[serde(default)]
    pub services: ServicesSpec,
}

impl ManifestV2 {
    /// Parse a manifest 2.0 document.
    pub fn parse(text: &str, origin: &Path) -> Result<Self, ExtensionError> {
        let manifest: Self = toml::from_str(text).map_err(|error| ExtensionError::Toml {
            path: origin.to_path_buf(),
            message: error.to_string(),
        })?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Read extension.toml from a package root.
    pub fn load(root: &Path) -> Result<Self, ExtensionError> {
        let path = root.join(MANIFEST_FILE);
        let text = std::fs::read_to_string(&path).map_err(|source| ExtensionError::Io {
            path: path.clone(),
            source,
        })?;
        if text.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(ExtensionError::Limit {
                extension: root.display().to_string(),
                message: "manifest is too large".to_string(),
            });
        }
        Self::parse(&text, &path)
    }

    /// Validate the envelope and contribution uniqueness.
    pub fn validate(&self) -> Result<(), ExtensionError> {
        let path = Path::new(MANIFEST_FILE).to_path_buf();
        if self.schema_version != MANIFEST_V2_SCHEMA_VERSION {
            return Err(ExtensionError::Manifest {
                path,
                message: format!(
                    "schema_version must be {MANIFEST_V2_SCHEMA_VERSION}, found {}",
                    self.schema_version
                ),
            });
        }
        if self.runtime.kind.trim().is_empty() {
            return Err(ExtensionError::Manifest {
                path,
                message: "runtime.kind is required".to_string(),
            });
        }
        let mut seen = std::collections::BTreeSet::new();
        for contribution in &self.contributions {
            if contribution.kind.trim().is_empty() {
                return Err(ExtensionError::Manifest {
                    path,
                    message: "contribution.kind is required".to_string(),
                });
            }
            let key = (contribution.kind.clone(), contribution.id.clone());
            if !seen.insert(key) {
                return Err(ExtensionError::Manifest {
                    path,
                    message: format!(
                        "duplicate contribution {}:{}",
                        contribution.kind, contribution.id
                    ),
                });
            }
        }
        Ok(())
    }

    /// Serialize back to TOML.
    pub fn to_toml(&self) -> Result<String, ExtensionError> {
        toml::to_string_pretty(self).map_err(|error| ExtensionError::Manifest {
            path: Path::new(MANIFEST_FILE).to_path_buf(),
            message: error.to_string(),
        })
    }

    /// Migrate a manifest 1.x document into the 2.0 envelope.
    ///
    /// Legacy contribution lists become generic entries with empty payloads;
    /// the host registry supplies each kind's payload schema.
    pub fn from_legacy(legacy: &Manifest) -> Self {
        let mut contributions = Vec::new();
        let groups: [(&str, &[ContributionId]); 13] = [
            ("tool", &legacy.contributes.tools),
            ("command", &legacy.contributes.commands),
            ("panel", &legacy.contributes.panels),
            ("prompt", &legacy.contributes.prompts),
            ("context", &legacy.contributes.contexts),
            ("skill", &legacy.contributes.skills),
            ("workflow", &legacy.contributes.workflows),
            ("language", &legacy.contributes.languages),
            ("integration", &legacy.contributes.integrations),
            ("model-provider", &legacy.contributes.model_providers),
            ("agent-provider", &legacy.contributes.agent_providers),
            ("theme", &legacy.contributes.themes),
            ("preset", &legacy.contributes.presets),
        ];
        for (kind, ids) in groups {
            for id in ids {
                contributions.push(ContributionSpecV2 {
                    kind: kind.to_string(),
                    id: id.clone(),
                    payload: empty_table(),
                });
            }
        }
        Self {
            schema_version: MANIFEST_V2_SCHEMA_VERSION.to_string(),
            extension: legacy.extension.clone(),
            compatibility: CompatibilityV2 {
                host: legacy.compatibility.bsh.clone(),
                extension_api: legacy.compatibility.extension_api.clone(),
            },
            runtime: RuntimeSpecV2 {
                kind: legacy.runtime.kind.as_str().to_string(),
                config: empty_table(),
            },
            capabilities: CapabilitiesV2 {
                required: legacy
                    .capabilities
                    .required
                    .iter()
                    .map(|capability| capability.as_str().to_string())
                    .collect(),
            },
            contributions,
            configuration: legacy.configuration.clone(),
            resources: legacy.resources.clone(),
            dependencies: legacy.dependencies.clone(),
            services: legacy.services.clone(),
        }
    }
}

/// Migrate the manifest of one package without writing it.
pub fn migrate_package_manifest(root: &Path) -> Result<ManifestV2, ExtensionError> {
    let path = root.join(MANIFEST_FILE);
    let text = std::fs::read_to_string(&path).map_err(|source| ExtensionError::Io {
        path: path.clone(),
        source,
    })?;
    let legacy = Manifest::parse(&text, &path)?;
    Ok(ManifestV2::from_legacy(&legacy))
}

/// Rewrite extension.toml as 2.0, keeping the original as extension.toml.v1.
///
/// Returns the migrated manifest path.
pub fn write_migrated_package(root: &Path) -> Result<std::path::PathBuf, ExtensionError> {
    let migrated = migrate_package_manifest(root)?;
    let text = migrated.to_toml()?;
    let manifest = root.join(MANIFEST_FILE);
    let backup = root.join("extension.toml.v1");
    if !backup.exists() {
        std::fs::copy(&manifest, &backup).map_err(|source| ExtensionError::Io {
            path: backup.clone(),
            source,
        })?;
    }
    crate::package::write_atomic(&manifest, text.as_bytes())?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn v2_rejects_unknown_top_level_fields() {
        let text = r#"
schema_version = "2.0"
unexpected = true
[extension]
id = "com.example.v2"
name = "V2"
version = "1.0.0"
description = "test"
[compatibility]
host = ">=3.0.0"
extension_api = ">=1.0.0"
[runtime]
kind = "builtin"
"#;
        assert!(ManifestV2::parse(text, Path::new("extension.toml")).is_err());
    }

    #[test]
    fn v2_rejects_duplicate_contributions() {
        let text = r#"
schema_version = "2.0"
[extension]
id = "com.example.v2"
name = "V2"
version = "1.0.0"
description = "test"
[compatibility]
host = ">=3.0.0"
extension_api = ">=1.0.0"
[runtime]
kind = "builtin"
[[contribution]]
kind = "tool"
id = "echo"
[[contribution]]
kind = "tool"
id = "echo"
"#;
        assert!(ManifestV2::parse(text, Path::new("extension.toml")).is_err());
    }

    #[test]
    fn legacy_hello_migrates_to_generic_contributions() {
        let root =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions/hello");
        let legacy = Manifest::load(&root).expect("legacy");
        let v2 = ManifestV2::from_legacy(&legacy);
        assert_eq!(v2.schema_version, "2.0");
        assert!(v2
            .contributions
            .iter()
            .any(|contribution| contribution.kind == "tool" && contribution.id.as_str() == "echo"));
        let text = v2.to_toml().expect("serialize");
        let reparsed = ManifestV2::parse(&text, Path::new("extension.toml")).expect("reparse");
        assert_eq!(reparsed.extension.id, legacy.extension.id);
    }

    #[test]
    fn writes_a_migrated_manifest_and_keeps_a_backup() {
        let source =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions/hello");
        let temp = tempfile::tempdir().expect("tempdir");
        crate::package::copy_tree(&source, temp.path()).expect("copy");
        let path = write_migrated_package(temp.path()).expect("migrate");
        assert!(temp.path().join("extension.toml.v1").exists());
        let v2 = ManifestV2::load(temp.path()).expect("v2");
        assert_eq!(v2.schema_version, MANIFEST_V2_SCHEMA_VERSION);
        assert_eq!(path, temp.path().join(MANIFEST_FILE));
    }
}
