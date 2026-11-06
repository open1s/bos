//! Versioned descriptions of what a capability offers.

use crate::id::{CapabilityId, CapabilityKind, SurfaceName, VersionSource};
use semver::Version;
use serde::{Deserialize, Serialize};

/// A permission a capability asks for, for example fs.read:workspace.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Permission(String);

impl Permission {
    /// Build a permission scope.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The scope as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Permission {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// What invoking a capability may do to the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffect {
    /// Reads state.
    Read,
    /// Changes state.
    Write,
    /// Reaches the network.
    Network,
    /// Starts a process.
    Process,
    /// Consumes a metered resource.
    Spend,
    /// Acts as a principal.
    Auth,
}

impl SideEffect {
    /// The stable lower-case name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Network => "network",
            Self::Process => "process",
            Self::Spend => "spend",
            Self::Auth => "auth",
        }
    }
}

impl Risk {
    /// Parse a lower-case risk class name.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "critical" => Some(Self::Critical),
            _ => None,
        }
    }
}

impl SideEffect {
    /// Parse a lower-case side-effect name.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "read" => Some(Self::Read),
            "write" => Some(Self::Write),
            "network" => Some(Self::Network),
            "process" => Some(Self::Process),
            "spend" => Some(Self::Spend),
            "auth" => Some(Self::Auth),
            _ => None,
        }
    }
}

/// How much damage a wrong or hostile invocation could do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// No meaningful side effect.
    Low,
    /// Bounded, local, recoverable.
    Medium,
    /// Wide or hard to reverse.
    High,
    /// Security or data-critical.
    Critical,
}

/// Who may see a descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// Publishable in discovery before any grant exists.
    Public,
    /// Visible only to a peer that holds a grant.
    Granted,
}

/// The shape of what a capability returns.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OutputContract {
    /// Content types the result may carry.
    pub content_types: Vec<String>,
    /// JSON Schema for structured output, when there is one.
    pub schema: Option<serde_json::Value>,
}

impl OutputContract {
    /// A contract for a JSON result with no declared schema.
    pub fn json() -> Self {
        Self {
            content_types: vec!["application/json".to_string()],
            schema: None,
        }
    }
}

/// Hard bounds the runtime enforces on one invocation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvocationLimits {
    /// Wall-clock ceiling, when the capability has one.
    pub timeout_ms: Option<u64>,
    /// Largest accepted input, in bytes.
    pub max_input_bytes: Option<u64>,
    /// Largest result, in bytes.
    pub max_output_bytes: Option<u64>,
    /// Most concurrent invocations.
    pub max_concurrency: Option<u16>,
}

/// Optional invocation features a capability supports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureSet {
    /// Partial output can be streamed.
    pub streaming: bool,
    /// A running invocation can be cancelled by id.
    pub cancellation: bool,
    /// Progress can be reported.
    pub progress: bool,
}

/// One versioned, described unit of functionality.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityDescriptor {
    /// Stable identity.
    pub id: CapabilityId,
    /// Semantic version of the contract, not of the implementation.
    pub version: Version,
    /// Where the version came from.
    pub version_source: VersionSource,
    /// What the capability does, semantically.
    pub kind: CapabilityKind,
    /// Human-readable name.
    pub display_name: String,
    /// Human-readable description. Untrusted until validated.
    pub description: String,
    /// JSON Schema for the arguments.
    pub input_schema: serde_json::Value,
    /// The shape of the result.
    pub output: OutputContract,
    /// Permission scopes a call needs.
    pub permissions: Vec<Permission>,
    /// What a call may do to the world.
    pub side_effects: Vec<SideEffect>,
    /// How much damage a wrong call could do.
    pub risk: Risk,
    /// Bounds the runtime enforces.
    pub limits: InvocationLimits,
    /// Optional features the capability supports.
    pub supports: FeatureSet,
    /// The name a model calls.
    pub surface: SurfaceName,
    /// Who may see this descriptor.
    pub visibility: Visibility,
}

impl CapabilityDescriptor {
    /// Start a descriptor with the safe defaults: medium risk, no permissions,
    /// no side effects, and visibility limited to holders of a grant.
    pub fn new(
        id: CapabilityId,
        kind: CapabilityKind,
        version: Version,
        version_source: VersionSource,
    ) -> Self {
        let surface = SurfaceName::new(id.name.clone());
        Self {
            id,
            version,
            version_source,
            kind,
            display_name: String::new(),
            description: String::new(),
            input_schema: serde_json::Value::Null,
            output: OutputContract::default(),
            permissions: Vec::new(),
            side_effects: Vec::new(),
            risk: Risk::Medium,
            limits: InvocationLimits::default(),
            supports: FeatureSet::default(),
            surface,
            visibility: Visibility::Granted,
        }
    }

    /// Set the display name.
    pub fn with_display_name(mut self, name: impl Into<String>) -> Self {
        self.display_name = name.into();
        self
    }

    /// Set the description a model reads.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// Set the argument schema.
    pub fn with_input_schema(mut self, schema: serde_json::Value) -> Self {
        self.input_schema = schema;
        self
    }

    /// Set the result contract.
    pub fn with_output(mut self, output: OutputContract) -> Self {
        self.output = output;
        self
    }

    /// Set the permission scopes.
    pub fn with_permissions(mut self, permissions: Vec<Permission>) -> Self {
        self.permissions = permissions;
        self
    }

    /// Set the side effects.
    pub fn with_side_effects(mut self, side_effects: Vec<SideEffect>) -> Self {
        self.side_effects = side_effects;
        self
    }

    /// Set the risk class.
    pub fn with_risk(mut self, risk: Risk) -> Self {
        self.risk = risk;
        self
    }

    /// Set the invocation bounds.
    pub fn with_limits(mut self, limits: InvocationLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Set the supported features.
    pub fn with_supports(mut self, supports: FeatureSet) -> Self {
        self.supports = supports;
        self
    }

    /// Set the name a model calls.
    pub fn with_surface(mut self, surface: SurfaceName) -> Self {
        self.surface = surface;
        self
    }

    /// Set who may see the descriptor.
    pub fn with_visibility(mut self, visibility: Visibility) -> Self {
        self.visibility = visibility;
        self
    }
}
