#![warn(missing_docs)]
//! Host-neutral core types for the BOS extension framework.
//!
//! This crate deliberately knows nothing about BSH, agents, GPUI or settings.
//! A host registers its vocabulary and the manager drives the same lifecycle
//! for every host.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

pub mod capability;
pub mod config;
pub mod events;
pub mod ui;
pub mod sources;
pub mod storage;
pub mod terminal;
#[cfg(feature = "tokio")]
#[allow(dead_code, missing_docs)] // Phase 1 move; docs and consumers land with the manager API.
pub mod composable;
pub mod dispatch;
pub mod manager;
pub mod host;
pub mod manifest;
pub mod manifest_v2;
pub mod package;
pub mod resolver;
pub mod signing;
pub mod types;
pub mod effects;

pub use dispatch::DispatchMode;

use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Manifest schema accepted by this framework.
pub const MANIFEST_SCHEMA_VERSION: &str = "2.0";

/// Extension API version exposed to packages.
pub const EXTENSION_API_VERSION: &str = "1.0";

/// One contribution produced by a runtime or declared by a manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Contribution {
    /// Host-registered kind, for example agent.tools or ui.panels.
    pub kind: String,
    /// Stable id inside the kind.
    pub id: String,
    /// Opaque payload validated by the kind schema.
    #[serde(default)]
    pub payload: Value,
}

/// A reference to one contribution.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContributionRef {
    /// Host-registered kind.
    pub kind: String,
    /// Stable id inside the kind.
    pub id: String,
}

/// Host-owned lease for one applied contribution.
#[async_trait::async_trait]
pub trait ContributionLease: Send + Sync {
    /// Remove the contribution. Must be idempotent.
    async fn revoke(&self) -> Result<(), ExtError>;
}

/// Host-owned sink that applies a contribution and returns its lease.
#[async_trait::async_trait]
pub trait ContributionSink: Send + Sync {
    /// Apply one contribution.
    async fn apply(
        &self,
        contribution: Contribution,
    ) -> Result<Box<dyn ContributionLease>, ExtError>;
}

/// One scope level. The kind is registered by the host.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Scope {
    /// Host-registered scope kind, for example workspace.
    pub kind: String,
    /// Scope instance id.
    pub id: String,
}

/// A path from a concrete scope to a wider scope, at most four levels.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScopePath(pub Vec<Scope>);

impl ScopePath {
    /// Build a path from the concrete level upward.
    pub fn new(scopes: Vec<Scope>) -> Self {
        Self(scopes)
    }

    /// The levels in this path.
    pub fn levels(&self) -> &[Scope] {
        &self.0
    }
}

/// Coarse risk bucket used by default policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Risk {
    /// Low risk.
    Low,
    /// Medium risk.
    Medium,
    /// High risk.
    High,
    /// Critical risk.
    Critical,
}

/// Host-registered capability metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilitySpec {
    /// Stable capability id.
    pub id: String,
    /// Human-readable description.
    pub description: String,
    /// Risk bucket.
    pub risk: Risk,
}

/// Registry of capability ids the host understands.
#[derive(Debug, Clone, Default)]
pub struct CapabilityRegistry {
    specs: BTreeMap<String, CapabilitySpec>,
}

impl CapabilityRegistry {
    /// Register one capability spec.
    pub fn register(&mut self, spec: CapabilitySpec) {
        self.specs.insert(spec.id.clone(), spec);
    }

    /// Look up a capability.
    pub fn get(&self, id: &str) -> Option<&CapabilitySpec> {
        self.specs.get(id)
    }

    /// Whether the id is registered.
    pub fn contains(&self, id: &str) -> bool {
        self.specs.contains_key(id)
    }
}

/// Host-registered service contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceContract {
    /// Stable contract id.
    pub contract: String,
    /// Contract version.
    pub version: String,
    /// Allowed scope kinds.
    pub scopes: Vec<String>,
}

/// Registry of service contracts the host understands.
#[derive(Debug, Clone, Default)]
pub struct ServiceContractRegistry {
    contracts: BTreeMap<String, ServiceContract>,
}

impl ServiceContractRegistry {
    /// Register one contract.
    pub fn register(&mut self, contract: ServiceContract) {
        self.contracts.insert(contract.contract.clone(), contract);
    }

    /// Look up a contract.
    pub fn get(&self, id: &str) -> Option<&ServiceContract> {
        self.contracts.get(id)
    }

    /// Whether the contract is registered.
    pub fn contains(&self, id: &str) -> bool {
        self.contracts.contains_key(id)
    }
}

/// Host-registered event metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSpec {
    /// Stable event name.
    pub name: String,
    /// Modes the host permits.
    pub modes: Vec<DispatchMode>,
    /// Optional interceptor capability.
    pub interceptor_capability: Option<String>,
}

/// One service availability transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceChange {
    /// Service contract id.
    pub contract: String,
    /// Scope where it changed.
    pub scope: Scope,
    /// Generation number.
    pub generation: u64,
    /// Whether it appeared, was replaced, or disappeared.
    pub available: bool,
}

/// Host-provided change stream for reactive service reload.
pub trait ChangeStream: Send + Sync {
    /// Subscribe to service changes.
    fn subscribe(&self) -> BoxStream<'static, ServiceChange>;
}

/// Host-owned event sink.
#[async_trait::async_trait]
pub trait EventSink: Send + Sync {
    /// Publish one event.
    async fn publish(&self, spec: &str, payload: Value) -> Result<(), ExtError>;
    /// Subscribe to one event.
    fn subscribe(&self, spec: &str) -> BoxStream<'static, Value>;
}

/// Host-owned audit sink.
pub trait AuditSink: Send + Sync {
    /// Record one decision.
    fn record(&self, entry: AuditEntry);
}

/// One audit record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Extension id.
    pub extension: String,
    /// Operation, for example install or capability.
    pub operation: String,
    /// Outcome.
    pub outcome: String,
    /// Free-form details.
    pub details: Value,
}

/// Stable error category.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    /// Manifest validation failed.
    Manifest,
    /// Package or store operation failed.
    Package,
    /// Policy denied the operation.
    Policy,
    /// Lifecycle transition failed.
    Lifecycle,
    /// Runtime failed.
    Runtime,
    /// Contribution application failed.
    Contribution,
    /// Service lookup or contract failed.
    Service,
    /// Event dispatch failed.
    Event,
    /// Protocol frame failed.
    Protocol,
    /// Host-defined code.
    Host(String),
}

/// One framework error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtError {
    /// Stable category.
    pub code: ErrorCode,
    /// Human-readable message.
    pub message: String,
    /// Structured details.
    #[serde(default)]
    pub details: Value,
}

impl fmt::Display for ExtError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for ExtError {}

/// Host configuration surface.
pub trait HostSpec: Send + Sync {
    /// The canonical store root.
    fn root(&self) -> PathBuf;
    /// The current scope path.
    fn scope_path(&self) -> ScopePath;
    /// Registered capabilities.
    fn capabilities(&self) -> &CapabilityRegistry;
    /// Registered service contracts.
    fn contracts(&self) -> &ServiceContractRegistry;
}

/// Runtime context passed to a started extension.
#[derive(Debug, Clone)]
pub struct RuntimeContext {
    /// Extension id.
    pub extension: String,
    /// Canonical store root.
    pub root: PathBuf,
    /// Extension data root.
    pub data_root: PathBuf,
}

/// One invocation context.
#[derive(Debug, Clone)]
pub struct InvokeContext {
    /// Correlation id.
    pub correlation: String,
    /// Per-call timeout in milliseconds.
    pub timeout_ms: u64,
}

/// Host-neutral runtime interface.
#[async_trait::async_trait]
pub trait Runtime: Send + Sync {
    /// Runtime kind id.
    fn kind(&self) -> &str;
    /// Start the runtime and describe its contributions.
    async fn start(&self, ctx: &RuntimeContext) -> Result<Vec<Contribution>, ExtError>;
    /// Invoke one contribution.
    async fn invoke(
        &self,
        contribution: &ContributionRef,
        input: Value,
        ctx: &InvokeContext,
    ) -> Result<Value, ExtError>;
    /// Shut the runtime down.
    async fn shutdown(&self) -> Result<(), ExtError>;
}

/// Host-owned factory that builds a runtime for one installed package.
pub trait RuntimeResolver: Send + Sync {
    /// Build the runtime for one manifest and package root.
    fn create(
        &self,
        manifest: &crate::manifest::Manifest,
        root: &std::path::Path,
        data_root: &std::path::Path,
    ) -> Result<Arc<dyn Runtime>, ExtError>;
}

/// Host-registered runtime kind constructor.
pub type RuntimeFactory = Arc<dyn Fn(&PathBuf) -> Result<Arc<dyn Runtime>, ExtError> + Send + Sync>;

/// Host-registered runtime kind.
pub struct RuntimeKindSpec {
    /// Runtime kind id.
    pub kind: String,
    /// Constructor for the runtime.
    pub create: RuntimeFactory,
}

impl fmt::Debug for RuntimeKindSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeKindSpec")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}
