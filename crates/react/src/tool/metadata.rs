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
}
