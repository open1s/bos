//! Stable identities: capabilities, providers, Nodes, and workspaces.
//!
//! A capability id is provider-qualified. Two providers may each ship a tool
//! called search, and they are different capabilities; a workspace view, an
//! audit row, and a remote binding all name the pair, never a bare name.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// A provider-scoped capability identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CapabilityId {
    /// The provider that offers the capability.
    pub provider: ProviderId,
    /// The provider-local name. Stable for the life of that provider.
    pub name: String,
}

/// Why an identity string was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid identity: {message}")]
pub struct InvalidId {
    /// What was wrong with the value.
    pub message: String,
}

impl InvalidId {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl CapabilityId {
    /// Build an id from a provider and a provider-local name.
    ///
    /// Neither part may be empty, and a slash is reserved as the separator, so
    /// it may appear in neither.
    pub fn new(
        provider: impl Into<ProviderId>,
        name: impl Into<String>,
    ) -> Result<Self, InvalidId> {
        let provider = provider.into();
        let name = name.into();
        if provider.as_str().is_empty() {
            return Err(InvalidId::new("the provider may not be empty"));
        }
        if provider.as_str().contains('/') {
            return Err(InvalidId::new("the provider may not contain '/'"));
        }
        if name.is_empty() {
            return Err(InvalidId::new("the capability name may not be empty"));
        }
        if name.contains('/') {
            return Err(InvalidId::new("the capability name may not contain '/'"));
        }
        Ok(Self { provider, name })
    }

    /// The provider that offers the capability.
    pub fn provider(&self) -> &ProviderId {
        &self.provider
    }

    /// The provider-local name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for CapabilityId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.provider, self.name)
    }
}

impl FromStr for CapabilityId {
    type Err = InvalidId;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let Some((provider, name)) = value.split_once('/') else {
            return Err(InvalidId::new(
                "a capability id is provider-qualified: <provider>/<name>",
            ));
        };
        Self::new(provider, name)
    }
}

/// One provider's stable name, for example builtin, mcp:github, or ferris:com.example.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProviderId(String);

impl ProviderId {
    /// Build a provider id from its wire form. It may not contain a slash.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The provider this Node implements natively.
    pub fn builtin() -> Self {
        Self::new("builtin")
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<&str> for ProviderId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for ProviderId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

/// The name a model calls, which may differ from the capability name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SurfaceName(String);

impl SurfaceName {
    /// Build a surface name.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SurfaceName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A Node's federation identity.
///
/// It is deliberately not a transport endpoint: a Node keeps its identity
/// across a transport change, and a transport id is not an identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NodeId(String);

impl NodeId {
    /// Build a Node id from its wire form. It may not be empty.
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidId> {
        let value = value.into();
        if value.is_empty() {
            return Err(InvalidId::new("a Node id may not be empty"));
        }
        Ok(Self(value))
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One instantiated Workspace instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WorkspaceId(String);

impl WorkspaceId {
    /// Build a workspace id from its wire form. It may not be empty.
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidId> {
        let value = value.into();
        if value.is_empty() {
            return Err(InvalidId::new("a workspace id may not be empty"));
        }
        Ok(Self(value))
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkspaceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// What a capability does, semantically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityKind {
    /// An atomic invocation.
    Tool,
    /// A reusable method or task procedure.
    Skill,
    /// A long-lived endpoint or process.
    Service,
    /// An addressable piece of data or context.
    Resource,
    /// A delegated decision-making principal.
    Agent,
    /// A declarative multi-step composition.
    Workflow,
}

impl CapabilityKind {
    /// The stable lower-case name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Skill => "skill",
            Self::Service => "service",
            Self::Resource => "resource",
            Self::Agent => "agent",
            Self::Workflow => "workflow",
        }
    }
}

impl fmt::Display for CapabilityKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Where a descriptor's version came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionSource {
    /// The provider declared it, for example in a package manifest.
    Declared,
    /// The host derived it from the implementation.
    Derived,
    /// No version exists. A range check against it is advisory at best.
    Unknown,
}
