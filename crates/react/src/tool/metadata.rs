//! Static metadata a tool declares about its capability contract.
//!
//! The tool author is the only one who knows how risky the body is, so the
//! declaration lives on the tool instead of a table maintained somewhere else.
//! A tool that declares nothing is described with conservative defaults and an
//! unknown version source; it is never described as safer than it is.

/// What a tool declares about itself, independent of any catalogue.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolMetadata {
    /// Contract version, semantic versioning. None means undeclared.
    pub version: Option<String>,
    /// Risk class: low, medium, high, or critical.
    pub risk: Option<String>,
    /// Side effects: read, write, network, process, spend, or auth.
    pub side_effects: Vec<String>,
    /// Permission scopes, for example fs.write:workspace.
    pub permissions: Vec<String>,
    /// Wall-clock ceiling in milliseconds.
    pub timeout_ms: Option<u64>,
    /// Result ceiling in bytes.
    pub max_output_bytes: Option<u64>,
    /// Discovery category, for example "files", "network" or "analysis".
    pub category: Option<String>,
    /// Free-form discovery tags.
    pub tags: Vec<String>,
    /// Intent or synonym terms a search may match.
    pub intents: Vec<String>,
    /// Stable capability identity, when this tool is a capability shim. The
    /// surface name stays the model-facing name; this is the runtime identity
    /// discovery reports alongside it.
    pub capability_id: Option<String>,
    /// The capability contract's semantic version.
    pub capability_version: Option<String>,
    /// The provider or peer that would execute the capability.
    pub provider: Option<String>,
}

impl ToolMetadata {
    /// Empty metadata, which is the same as declaring nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare the contract version.
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// Declare the risk class.
    pub fn with_risk(mut self, risk: impl Into<String>) -> Self {
        self.risk = Some(risk.into());
        self
    }

    /// Declare side effects.
    pub fn with_side_effects(
        mut self,
        effects: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.side_effects = effects.into_iter().map(Into::into).collect();
        self
    }

    /// Declare permission scopes.
    pub fn with_permissions(
        mut self,
        permissions: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.permissions = permissions.into_iter().map(Into::into).collect();
        self
    }

    /// Declare the wall-clock ceiling.
    pub fn with_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    /// Declare the result ceiling.
    pub fn with_max_output_bytes(mut self, max_output_bytes: u64) -> Self {
        self.max_output_bytes = Some(max_output_bytes);
        self
    }

    /// Declare the discovery category.
    pub fn with_category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }

    /// Declare discovery tags.
    pub fn with_tags(mut self, tags: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.tags = tags.into_iter().map(Into::into).collect();
        self
    }

    /// Declare intent or synonym terms.
    pub fn with_intents(mut self, intents: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.intents = intents.into_iter().map(Into::into).collect();
        self
    }

    /// Declare the stable capability identity and contract version.
    pub fn with_capability(mut self, id: impl Into<String>, version: impl Into<String>) -> Self {
        self.capability_id = Some(id.into());
        self.capability_version = Some(version.into());
        self
    }

    /// Declare the provider or peer that executes the capability.
    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = Some(provider.into());
        self
    }
}
