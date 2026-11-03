//! Typed 2.0 manifest authoring.

use std::path::Path;

use ferris_core::manifest::{
    ConfigurationSpec, ResourcesSpec, ServiceProvideSpec, ServiceRequireSpec,
};
use ferris_core::manifest_v2::ManifestV2;
use ferris_core::types::StartupPolicy;

use crate::contribution::ContributionSpec;
use crate::error::{invalid_input, Result};

/// A validated 2.0 manifest.
#[derive(Debug, Clone)]
pub struct ExtensionManifest {
    inner: ManifestV2,
    toml: String,
}

impl ExtensionManifest {
    /// Parse and validate a 2.0 manifest.
    pub fn parse(text: &str) -> Result<Self> {
        let inner = ManifestV2::parse(text, Path::new("extension.toml"))
            .map_err(|error| invalid_input(error.to_string()))?;
        Ok(Self {
            inner,
            toml: text.to_string(),
        })
    }

    /// The parsed manifest.
    pub fn as_v2(&self) -> &ManifestV2 {
        &self.inner
    }

    /// The canonical TOML.
    pub fn to_toml(&self) -> &str {
        &self.toml
    }

    /// Extension id.
    pub fn id(&self) -> &ferris_core::types::ExtensionId {
        &self.inner.extension.id
    }

    /// Runtime kind.
    pub fn runtime_kind(&self) -> &str {
        &self.inner.runtime.kind
    }
}

/// Fluent builder for a 2.0 manifest.
#[derive(Debug, Clone)]
pub struct ManifestBuilder {
    id: String,
    name: String,
    version: String,
    description: String,
    authors: Vec<String>,
    license: Option<String>,
    repository: Option<String>,
    homepage: Option<String>,
    host: String,
    extension_api: String,
    runtime: String,
    runtime_config: toml::map::Map<String, toml::Value>,
    capabilities: Vec<String>,
    contributions: Vec<ContributionSpec>,
    dependencies: std::collections::BTreeMap<String, String>,
    provides: Vec<ServiceProvideSpec>,
    requires: Vec<ServiceRequireSpec>,
    resources: ResourcesSpec,
    configuration: Option<ConfigurationSpec>,
}

impl ManifestBuilder {
    /// Start a manifest with the required identity fields.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        version: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            version: version.into(),
            description: description.into(),
            authors: Vec::new(),
            license: None,
            repository: None,
            homepage: None,
            host: ">=3.0.0".to_string(),
            extension_api: ">=1.0.0".to_string(),
            runtime: "declarative".to_string(),
            runtime_config: toml::map::Map::new(),
            capabilities: Vec::new(),
            contributions: Vec::new(),
            dependencies: std::collections::BTreeMap::new(),
            provides: Vec::new(),
            requires: Vec::new(),
            resources: ResourcesSpec::default(),
            configuration: None,
        }
    }

    /// Host version range.
    #[must_use]
    pub fn host(mut self, requirement: impl Into<String>) -> Self {
        self.host = requirement.into();
        self
    }

    /// Extension API range.
    #[must_use]
    pub fn extension_api(mut self, requirement: impl Into<String>) -> Self {
        self.extension_api = requirement.into();
        self
    }

    /// Runtime kind.
    #[must_use]
    pub fn runtime(mut self, kind: impl Into<String>) -> Self {
        self.runtime = kind.into();
        self
    }

    /// Set one opaque runtime config value.
    #[must_use]
    pub fn runtime_config(mut self, key: impl Into<String>, value: toml::Value) -> Self {
        self.runtime_config.insert(key.into(), value);
        self
    }

    /// Add one required capability.
    #[must_use]
    pub fn capability(mut self, capability: impl Into<String>) -> Self {
        self.capabilities.push(capability.into());
        self
    }

    /// Add one contribution.
    #[must_use]
    pub fn contribution(mut self, contribution: ContributionSpec) -> Self {
        self.contributions.push(contribution);
        self
    }

    /// Add one author.
    #[must_use]
    pub fn author(mut self, author: impl Into<String>) -> Self {
        self.authors.push(author.into());
        self
    }

    /// Set the SPDX licence.
    #[must_use]
    pub fn license(mut self, license: impl Into<String>) -> Self {
        self.license = Some(license.into());
        self
    }

    /// Set the repository URL.
    #[must_use]
    pub fn repository(mut self, repository: impl Into<String>) -> Self {
        self.repository = Some(repository.into());
        self
    }

    /// Set the homepage.
    #[must_use]
    pub fn homepage(mut self, homepage: impl Into<String>) -> Self {
        self.homepage = Some(homepage.into());
        self
    }

    /// Add one dependency range.
    #[must_use]
    pub fn dependency(mut self, id: impl Into<String>, requirement: impl Into<String>) -> Self {
        self.dependencies.insert(id.into(), requirement.into());
        self
    }

    /// Add one provided service contract.
    #[must_use]
    pub fn provide(mut self, service: ServiceProvideSpec) -> Self {
        self.provides.push(service);
        self
    }

    /// Add one required service contract.
    #[must_use]
    pub fn require(mut self, service: ServiceRequireSpec) -> Self {
        self.requires.push(service);
        self
    }

    /// Set the memory ceiling in MiB.
    #[must_use]
    pub fn max_memory_mb(mut self, value: u64) -> Self {
        self.resources.max_memory_mb = Some(value);
        self
    }

    /// Set the execution ceiling in milliseconds.
    #[must_use]
    pub fn max_execution_ms(mut self, value: u64) -> Self {
        self.resources.max_execution_ms = Some(value);
        self
    }

    /// Set the concurrency ceiling.
    #[must_use]
    pub fn max_concurrent_calls(mut self, value: u32) -> Self {
        self.resources.max_concurrent_calls = Some(value);
        self
    }

    /// Set the output ceiling in bytes.
    #[must_use]
    pub fn max_output_bytes(mut self, value: u64) -> Self {
        self.resources.max_output_bytes = Some(value);
        self
    }

    /// Set the package-relative JSON Schema path.
    #[must_use]
    pub fn configuration_schema(mut self, path: impl Into<String>) -> Self {
        self.configuration
            .get_or_insert_with(Default::default)
            .schema = Some(path.into());
        self
    }

    /// Set configuration defaults.
    #[must_use]
    pub fn configuration_defaults(mut self, defaults: toml::Value) -> Self {
        self.configuration
            .get_or_insert_with(Default::default)
            .defaults = Some(defaults);
        self
    }

    /// Set the activation policy stored in the manifest's runtime config.
    #[must_use]
    pub fn startup(mut self, startup: StartupPolicy) -> Self {
        let value = match startup {
            StartupPolicy::OnStartup => "on-startup",
            StartupPolicy::OnDemand => "on-demand",
            StartupPolicy::OnWorkspaceOpen => "on-workspace-open",
            StartupPolicy::Manual => "manual",
        };
        self.runtime_config.insert(
            "startup".to_string(),
            toml::Value::String(value.to_string()),
        );
        self
    }

    /// Render and validate the manifest.
    pub fn build(self) -> Result<ExtensionManifest> {
        let mut extension = toml::map::Map::new();
        extension.insert("id".to_string(), toml::Value::String(self.id));
        extension.insert("name".to_string(), toml::Value::String(self.name));
        extension.insert("version".to_string(), toml::Value::String(self.version));
        extension.insert(
            "description".to_string(),
            toml::Value::String(self.description),
        );
        if !self.authors.is_empty() {
            extension.insert(
                "authors".to_string(),
                toml::Value::Array(self.authors.into_iter().map(toml::Value::String).collect()),
            );
        }
        if let Some(license) = self.license {
            extension.insert("license".to_string(), toml::Value::String(license));
        }
        if let Some(repository) = self.repository {
            extension.insert("repository".to_string(), toml::Value::String(repository));
        }
        if let Some(homepage) = self.homepage {
            extension.insert("homepage".to_string(), toml::Value::String(homepage));
        }

        let mut compatibility = toml::map::Map::new();
        compatibility.insert("host".to_string(), toml::Value::String(self.host));
        compatibility.insert(
            "extension_api".to_string(),
            toml::Value::String(self.extension_api),
        );

        let mut runtime = toml::map::Map::new();
        runtime.insert("kind".to_string(), toml::Value::String(self.runtime));
        if !self.runtime_config.is_empty() {
            runtime.insert(
                "config".to_string(),
                toml::Value::Table(self.runtime_config),
            );
        }

        let mut capabilities = toml::map::Map::new();
        capabilities.insert(
            "required".to_string(),
            toml::Value::Array(
                self.capabilities
                    .into_iter()
                    .map(toml::Value::String)
                    .collect(),
            ),
        );

        let mut root = toml::map::Map::new();
        root.insert(
            "schema_version".to_string(),
            toml::Value::String(ferris_core::manifest_v2::MANIFEST_V2_SCHEMA_VERSION.to_string()),
        );
        root.insert("extension".to_string(), toml::Value::Table(extension));
        root.insert(
            "compatibility".to_string(),
            toml::Value::Table(compatibility),
        );
        root.insert("runtime".to_string(), toml::Value::Table(runtime));
        root.insert("capabilities".to_string(), toml::Value::Table(capabilities));

        if !self.dependencies.is_empty() {
            let mut dependencies = toml::map::Map::new();
            for (id, requirement) in self.dependencies {
                dependencies.insert(id, toml::Value::String(requirement));
            }
            root.insert("dependencies".to_string(), toml::Value::Table(dependencies));
        }
        if !self.provides.is_empty() || !self.requires.is_empty() {
            let mut services = toml::map::Map::new();
            if !self.provides.is_empty() {
                services.insert(
                    "provides".to_string(),
                    toml::Value::Array(
                        self.provides
                            .into_iter()
                            .map(|service| {
                                toml::Value::try_from(service)
                                    .map_err(|error| invalid_input(error.to_string()))
                            })
                            .collect::<Result<Vec<_>>>()?,
                    ),
                );
            }
            if !self.requires.is_empty() {
                services.insert(
                    "requires".to_string(),
                    toml::Value::Array(
                        self.requires
                            .into_iter()
                            .map(|service| {
                                toml::Value::try_from(service)
                                    .map_err(|error| invalid_input(error.to_string()))
                            })
                            .collect::<Result<Vec<_>>>()?,
                    ),
                );
            }
            root.insert("services".to_string(), toml::Value::Table(services));
        }
        let resources = toml::Value::try_from(self.resources)
            .map_err(|error| invalid_input(error.to_string()))?;
        if let toml::Value::Table(resources) = resources {
            if !resources.is_empty() {
                root.insert("resources".to_string(), toml::Value::Table(resources));
            }
        }
        if let Some(configuration) = self.configuration {
            let configuration = toml::Value::try_from(configuration)
                .map_err(|error| invalid_input(error.to_string()))?;
            root.insert("configuration".to_string(), configuration);
        }
        root.insert(
            "contribution".to_string(),
            toml::Value::Array(
                self.contributions
                    .into_iter()
                    .map(|contribution| toml::Value::Table(contribution.into_table()))
                    .collect(),
            ),
        );

        let text = toml::to_string_pretty(&toml::Value::Table(root))
            .map_err(|error| invalid_input(error.to_string()))?;
        ExtensionManifest::parse(&text)
    }
}
