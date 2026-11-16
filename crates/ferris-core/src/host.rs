//! Host adapter bundle.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use crate::{AuditSink, ContributionSink, EventSink, ExtError, HostSpec};

/// A host-provided task spawner.
pub trait Spawner: Send + Sync {
    /// Spawn a detached future.
    fn spawn(&self, future: futures::future::BoxFuture<'static, ()>);
}

/// One process sandbox request.
#[derive(Debug, Clone)]
pub struct SandboxSpec {
    /// Package root.
    pub root: PathBuf,
    /// Workspace root.
    pub workspace: Option<PathBuf>,
    /// Extension data root.
    pub data_root: Option<PathBuf>,
    /// Program to execute.
    pub program: PathBuf,
    /// Program arguments.
    pub args: Vec<String>,
    /// Optional memory ceiling.
    pub memory_bytes: Option<u64>,
    /// Optional CPU ceiling.
    pub cpu_seconds: Option<u64>,
}

/// The wrapped command a sandbox produces.
#[derive(Debug, Clone)]
pub struct SandboxCommand {
    /// Program to execute.
    pub program: String,
    /// Arguments, including any sandbox wrapper.
    pub args: Vec<String>,
}

/// Host-owned process sandbox.
pub trait ProcessSandbox: Send + Sync {
    /// Wrap one process invocation.
    fn wrap(&self, spec: &SandboxSpec) -> Result<SandboxCommand, ExtError>;
}

/// Host-owned publisher trust roots.
pub trait TrustRoots: Send + Sync {
    /// Whether a publisher key is trusted.
    fn is_trusted(&self, publisher: &str) -> bool;
}

/// Host-owned remote registry.
#[async_trait::async_trait]
pub trait Registry: Send + Sync {
    /// Fetch one package archive.
    async fn fetch(&self, id: &str, version: &str) -> Result<Vec<u8>, ExtError>;
    /// Search the registry.
    async fn search(&self, query: &str) -> Result<Vec<Value>, ExtError>;
}

/// The complete host adapter bundle used by the manager.
pub struct Host {
    /// Host facts.
    pub spec: Arc<dyn HostSpec>,
    /// Contribution sink.
    pub contributions: Option<Arc<dyn ContributionSink>>,
    /// Event sink.
    pub events: Option<Arc<dyn EventSink>>,
    /// Audit sink.
    pub audit: Option<Arc<dyn AuditSink>>,
    /// Task spawner.
    pub spawner: Option<Arc<dyn Spawner>>,
    /// Process sandbox.
    pub sandbox: Option<Arc<dyn ProcessSandbox>>,
    /// Trust roots.
    pub trust: Option<Arc<dyn TrustRoots>>,
    /// Remote registry.
    pub registry: Option<Arc<dyn Registry>>,
}

impl Host {
    /// Build a host with only its facts; ports can be attached later.
    pub fn new(spec: Arc<dyn HostSpec>) -> Self {
        Self {
            spec,
            contributions: None,
            events: None,
            audit: None,
            spawner: None,
            sandbox: None,
            trust: None,
            registry: None,
        }
    }
}
