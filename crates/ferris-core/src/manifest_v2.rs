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

fn runtime_config(legacy: &Manifest) -> toml::Value {
    let mut table = toml::map::Map::new();
    if let Some(entry) = &legacy.runtime.entry {
        table.insert("entry".to_string(), toml::Value::String(entry.clone()));
    }
    if !legacy.runtime.args.is_empty() {
        table.insert(
            "args".to_string(),
            toml::Value::Array(
                legacy
                    .runtime
                    .args
                    .iter()
                    .cloned()
                    .map(toml::Value::String)
                    .collect(),
            ),
        );
    }
    if !legacy.runtime.env.is_empty() {
        let mut env = toml::map::Map::new();
        for (key, value) in &legacy.runtime.env {
            env.insert(key.clone(), toml::Value::String(value.clone()));
        }
        table.insert("env".to_string(), toml::Value::Table(env));
    }
    if let Some(protocol) = &legacy.runtime.protocol {
        table.insert(
            "protocol".to_string(),
            toml::Value::String(protocol.clone()),
        );
    }
    let startup = match legacy.runtime.startup {
        crate::types::StartupPolicy::OnStartup => "on-startup",
        crate::types::StartupPolicy::OnDemand => "on-demand",
        crate::types::StartupPolicy::OnWorkspaceOpen => "on-workspace-open",
        crate::types::StartupPolicy::Manual => "manual",
    };
    table.insert(
        "startup".to_string(),
        toml::Value::String(startup.to_string()),
    );
    toml::Value::Table(table)
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
        for topic in &legacy.contributes.events {
            if let Ok(id) = ContributionId::parse(topic.clone()) {
                contributions.push(ContributionSpecV2 {
                    kind: "event.subscribe".to_string(),
                    id,
                    payload: empty_table(),
                });
            }
        }
        for topic in &legacy.contributes.publishes {
            if let Ok(id) = ContributionId::parse(topic.clone()) {
                contributions.push(ContributionSpecV2 {
                    kind: "event.publish".to_string(),
                    id,
                    payload: empty_table(),
                });
            }
        }
        for point in &legacy.contributes.hooks {
            if let Ok(id) = ContributionId::parse(point.clone()) {
                contributions.push(ContributionSpecV2 {
                    kind: "hook".to_string(),
                    id,
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
                config: runtime_config(legacy),
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

    /// Convert a v2 manifest back into the internal 1.x shape.
    ///
    /// This is the transitional adapter: the host still runs the 1.x manifest
    /// model until the runtime and store migrate to generic contributions.
    pub fn into_legacy(self) -> Result<Manifest, ExtensionError> {
        use crate::capability::Capability;
        use crate::manifest::{CapabilitiesSpec, Compatibility, ContributesSpec, RuntimeSpec};
        use crate::types::RuntimeKind;

        let mut contributes = ContributesSpec::default();
        for contribution in self.contributions {
            let id = contribution.id;
            match contribution.kind.as_str() {
                "tool" => contributes.tools.push(id),
                "command" => contributes.commands.push(id),
                "panel" => contributes.panels.push(id),
                "prompt" => contributes.prompts.push(id),
                "context" => contributes.contexts.push(id),
                "skill" => contributes.skills.push(id),
                "workflow" => contributes.workflows.push(id),
                "language" => contributes.languages.push(id),
                "integration" => contributes.integrations.push(id),
                "model-provider" => contributes.model_providers.push(id),
                "agent-provider" => contributes.agent_providers.push(id),
                "theme" => contributes.themes.push(id),
                "preset" => contributes.presets.push(id),
                "event.subscribe" => contributes.events.push(id.as_str().to_string()),
                "event.publish" => contributes.publishes.push(id.as_str().to_string()),
                "hook" => contributes.hooks.push(id.as_str().to_string()),
                other => {
                    return Err(ExtensionError::Manifest {
                        path: Path::new(MANIFEST_FILE).to_path_buf(),
                        message: format!("unknown contribution kind {other:?}"),
                    })
                }
            }
        }
        let mut capabilities = CapabilitiesSpec::default();
        for capability in &self.capabilities.required {
            capabilities
                .required
                .push(capability.parse::<Capability>()?);
        }
        let config = &self.runtime.config;
        let entry = config
            .get("entry")
            .and_then(toml::Value::as_str)
            .map(str::to_string);
        let protocol = config
            .get("protocol")
            .and_then(toml::Value::as_str)
            .map(str::to_string);
        let args = config
            .get("args")
            .and_then(toml::Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(toml::Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let env = config
            .get("env")
            .and_then(toml::Value::as_table)
            .map(|table| {
                table
                    .iter()
                    .filter_map(|(key, value)| {
                        value.as_str().map(|value| (key.clone(), value.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let startup = match config.get("startup").and_then(toml::Value::as_str) {
            Some("on-startup") => crate::types::StartupPolicy::OnStartup,
            Some("on-workspace-open") => crate::types::StartupPolicy::OnWorkspaceOpen,
            Some("manual") => crate::types::StartupPolicy::Manual,
            _ => crate::types::StartupPolicy::OnDemand,
        };
        Ok(Manifest {
            schema_version: crate::types::MANIFEST_SCHEMA_VERSION.to_string(),
            extension: self.extension,
            compatibility: Compatibility {
                bsh: self.compatibility.host,
                extension_api: self.compatibility.extension_api,
                ui_api: None,
            },
            runtime: RuntimeSpec {
                kind: RuntimeKind::parse(&self.runtime.kind)?,
                entry,
                args,
                env,
                startup,
                protocol,
            },
            capabilities,
            contributes,
            configuration: self.configuration,
            resources: self.resources,
            dependencies: self.dependencies,
            services: self.services,
        })
    }
}

/// Load a package manifest in either 1.x or 2.0 form.
///
/// The 2.0 envelope is converted to the transitional host shape until the
/// store and runtimes consume generic contributions directly.
pub fn load_package_manifest(root: &Path) -> Result<Manifest, ExtensionError> {
    let path = root.join(MANIFEST_FILE);
    let text = std::fs::read_to_string(&path).map_err(|source| ExtensionError::Io {
        path: path.clone(),
        source,
    })?;
    let value: toml::Value = toml::from_str(&text).map_err(|error| ExtensionError::Toml {
        path: path.clone(),
        message: error.to_string(),
    })?;
    let schema = value
        .get("schema_version")
        .and_then(toml::Value::as_str)
        .unwrap_or("");
    if schema == MANIFEST_V2_SCHEMA_VERSION {
        ManifestV2::parse(&text, &path)?.into_legacy()
    } else {
        Manifest::parse(&text, &path)
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

    #[test]
    fn a_migrated_package_loads_through_the_host_shape() {
        let source =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions/hello");
        let temp = tempfile::tempdir().expect("tempdir");
        crate::package::copy_tree(&source, temp.path()).expect("copy");
        write_migrated_package(temp.path()).expect("migrate");
        let package = crate::package::Package::load(
            temp.path(),
            crate::types::ExtensionSource::Development,
            crate::types::TrustLevel::Local,
        )
        .expect("package");
        assert!(package.report.is_valid(), "{:?}", package.report.errors);
        assert!(package
            .manifest
            .contributes
            .tools
            .iter()
            .any(|id| id.as_str() == "echo"));
    }

    #[test]
    fn process_runtime_config_survives_migration() {
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/extensions/process-echo");
        let temp = tempfile::tempdir().expect("tempdir");
        crate::package::copy_tree(&source, temp.path()).expect("copy");
        write_migrated_package(temp.path()).expect("migrate");
        let v2 = ManifestV2::load(temp.path()).expect("v2");
        assert_eq!(
            v2.runtime.config.get("entry").and_then(toml::Value::as_str),
            Some("python3")
        );
        let legacy = load_package_manifest(temp.path()).expect("load");
        assert_eq!(legacy.runtime.entry.as_deref(), Some("python3"));
        assert_eq!(legacy.runtime.args, vec!["server.py".to_string()]);
    }

    #[test]
    fn event_contributions_survive_migration() {
        let source =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions/hook-demo");
        let temp = tempfile::tempdir().expect("tempdir");
        crate::package::copy_tree(&source, temp.path()).expect("copy");
        write_migrated_package(temp.path()).expect("migrate");
        let v2 = ManifestV2::load(temp.path()).expect("v2");
        assert_eq!(
            v2.contributions
                .iter()
                .filter(|contribution| contribution.kind == "event.subscribe")
                .count(),
            20
        );
        let legacy = load_package_manifest(temp.path()).expect("load");
        assert_eq!(legacy.contributes.events.len(), 20);
        assert_eq!(
            legacy.contributes.publishes,
            vec!["tool.completed".to_string()]
        );
    }

    #[test]
    fn hook_contributions_survive_migration() {
        let source =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions/extism-hook");
        let temp = tempfile::tempdir().expect("tempdir");
        crate::package::copy_tree(&source, temp.path()).expect("copy");
        write_migrated_package(temp.path()).expect("migrate");
        let v2 = ManifestV2::load(temp.path()).expect("v2");
        assert_eq!(
            v2.contributions
                .iter()
                .filter(|contribution| contribution.kind == "hook")
                .count(),
            3
        );
        let legacy = load_package_manifest(temp.path()).expect("load");
        assert_eq!(legacy.contributes.hooks.len(), 3);
    }
}
