//! Host-neutral helpers shared by the Python and Node.js Ferris bindings.
//!
//! The language crates own only FFI: callback adapters, napi/pyo3 classes and
//! their error mapping. The authoring helpers are always compiled; the host
//! assembly is behind the `host` feature so a slim addon does not link the
//! standard runtimes.

#![warn(missing_docs)]

use std::path::PathBuf;

use ferris_sdk::capability::{Capability, GrantScope, PolicyMode};
use ferris_sdk::toml;
use ferris_sdk::types::TrustLevel;

/// Convert a JSON value into a TOML value for runtime configuration and
/// contribution payloads.
pub fn json_to_toml(value: &serde_json::Value) -> Result<toml::Value, String> {
    match value {
        serde_json::Value::Null => Err("TOML has no null value".to_string()),
        serde_json::Value::Bool(value) => Ok(toml::Value::Boolean(*value)),
        serde_json::Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                Ok(toml::Value::Integer(integer))
            } else if let Some(float) = number.as_f64() {
                Ok(toml::Value::Float(float))
            } else {
                Err("unsupported TOML number".to_string())
            }
        }
        serde_json::Value::String(value) => Ok(toml::Value::String(value.clone())),
        serde_json::Value::Array(values) => values
            .iter()
            .map(json_to_toml)
            .collect::<Result<Vec<_>, _>>()
            .map(toml::Value::Array),
        serde_json::Value::Object(map) => {
            let mut table = toml::map::Map::new();
            for (key, value) in map {
                table.insert(key.clone(), json_to_toml(value)?);
            }
            Ok(toml::Value::Table(table))
        }
    }
}

/// Parse a startup policy name.
pub fn startup_policy(raw: &str) -> Result<ferris_sdk::StartupPolicy, String> {
    match raw.to_ascii_lowercase().replace('_', "-").as_str() {
        "on-startup" => Ok(ferris_sdk::StartupPolicy::OnStartup),
        "on-workspace-open" => Ok(ferris_sdk::StartupPolicy::OnWorkspaceOpen),
        "manual" => Ok(ferris_sdk::StartupPolicy::Manual),
        "on-demand" | "ondemand" => Ok(ferris_sdk::StartupPolicy::OnDemand),
        other => Err(format!("unknown startup policy: {other}")),
    }
}

/// Parse a trust level name.
pub fn parse_trust(raw: Option<&str>) -> Result<TrustLevel, String> {
    match raw.unwrap_or("community").to_ascii_lowercase().as_str() {
        "builtin" => Ok(TrustLevel::Builtin),
        "verified" => Ok(TrustLevel::Verified),
        "community" => Ok(TrustLevel::Community),
        "local" => Ok(TrustLevel::Local),
        "unknown" => Ok(TrustLevel::Unknown),
        other => Err(format!("unknown trust level: {other}")),
    }
}

/// Parse a policy mode name.
pub fn parse_policy_mode(raw: Option<&str>) -> Result<PolicyMode, String> {
    match raw.unwrap_or("enforce").to_ascii_lowercase().as_str() {
        "enforce" => Ok(PolicyMode::Enforce),
        "advisory" => Ok(PolicyMode::Advisory),
        other => Err(format!("unknown policy mode: {other}")),
    }
}

/// Parse a capability id.
pub fn parse_capability(raw: &str) -> Result<Capability, String> {
    raw.parse::<Capability>().map_err(|error| error.to_string())
}

/// Parse a grant scope. An unknown scope is an error, never a global grant.
pub fn parse_scope(raw: Option<&str>) -> Result<GrantScope, String> {
    let Some(raw) = raw else {
        return Ok(GrantScope::Any);
    };
    if raw == "any" {
        return Ok(GrantScope::Any);
    }
    if raw == "workspace" {
        return Ok(GrantScope::Workspace);
    }
    if let Some(path) = raw.strip_prefix("path:") {
        return Ok(GrantScope::Path(PathBuf::from(path)));
    }
    if let Some(domain) = raw.strip_prefix("domain:") {
        return Ok(GrantScope::Domain(domain.to_string()));
    }
    if let Some(program) = raw.strip_prefix("program:") {
        return Ok(GrantScope::Program(program.to_string()));
    }
    if let Some(secret) = raw.strip_prefix("secret:") {
        return Ok(GrantScope::Secret(secret.to_string()));
    }
    Err(format!("unknown grant scope: {raw}"))
}

#[cfg(feature = "host")]
mod host {
    use super::*;

    use std::any::Any;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};

    use async_trait::async_trait;

    use ferris_sdk::capability::PolicyEngine;
    use ferris_sdk::core_manifest::Manifest;
    use ferris_sdk::events::EventBus;
    use ferris_sdk::host_runtime::factory::{build, RuntimePorts};
    use ferris_sdk::terminal::TerminalService;
    use ferris_sdk::types::RuntimeKind;
    use ferris_sdk::{
        CapabilityRegistry, CapabilitySpec, Contribution, ContributionLease, ContributionSink,
        ExtError, HostSpec, LeaseHost, Runtime, RuntimeResolver, ScopePath,
        ServiceContractRegistry,
    };

    /// The host facts shared by both bindings.
    pub struct BindingHostSpec {
        root: PathBuf,
        capabilities: CapabilityRegistry,
        contracts: ServiceContractRegistry,
    }

    impl BindingHostSpec {
        /// Build the host spec and seed every SDK capability.
        pub fn new(root: PathBuf) -> Self {
            let mut capabilities = CapabilityRegistry::default();
            for capability in Capability::ALL {
                capabilities.register(CapabilitySpec {
                    id: capability.as_str().to_string(),
                    description: capability.description().to_string(),
                    risk: match capability.risk() {
                        ferris_sdk::capability::Risk::Low => ferris_sdk::Risk::Low,
                        ferris_sdk::capability::Risk::Medium => ferris_sdk::Risk::Medium,
                        ferris_sdk::capability::Risk::High => ferris_sdk::Risk::High,
                        ferris_sdk::capability::Risk::Critical => ferris_sdk::Risk::Critical,
                    },
                });
            }
            Self {
                root,
                capabilities,
                contracts: ServiceContractRegistry::default(),
            }
        }
    }

    impl HostSpec for BindingHostSpec {
        fn root(&self) -> PathBuf {
            self.root.clone()
        }

        fn scope_path(&self) -> ScopePath {
            ScopePath::default()
        }

        fn capabilities(&self) -> &CapabilityRegistry {
            &self.capabilities
        }

        fn contracts(&self) -> &ServiceContractRegistry {
            &self.contracts
        }
    }

    /// Ports and policy for the standard runtime factory.
    pub struct BindingRuntimeOptions {
        /// Open workspace, when any.
        pub workspace: Option<PathBuf>,
        /// Authorization engine.
        pub policy: Arc<PolicyEngine>,
        /// Host event bus.
        pub events: Arc<EventBus>,
        /// Package trust level.
        pub trust: TrustLevel,
        /// Terminal used by Cordis host calls.
        pub terminal: Arc<dyn TerminalService>,
        /// Historical switch for the platform OS sandbox. Ferris runtimes are
        /// no longer OS-sandboxed, so this no longer changes behavior; it stays
        /// for API compatibility with the bindings.
        pub process_sandbox: bool,
    }

    /// Resolve standard Ferris runtimes for a binding host.
    pub struct BindingRuntimeResolver {
        options: BindingRuntimeOptions,
    }

    impl BindingRuntimeResolver {
        /// Wrap the runtime ports.
        pub fn new(options: BindingRuntimeOptions) -> Self {
            Self { options }
        }
    }

    impl RuntimeResolver for BindingRuntimeResolver {
        fn create(
            &self,
            manifest: &Manifest,
            root: &std::path::Path,
            data_root: &std::path::Path,
        ) -> Result<Arc<dyn Runtime>, ExtError> {
            if manifest.runtime.kind == RuntimeKind::Builtin {
                return Err(ExtError {
                    code: ferris_sdk::ErrorCode::Runtime,
                    message: format!("builtin {} is not linked into this host", manifest.id()),
                    details: serde_json::Value::Null,
                });
            }
            let _ = std::fs::create_dir_all(data_root);
            if manifest.runtime.kind == RuntimeKind::Process && !self.options.process_sandbox {
                let runtime =
                    ferris_sdk::host_runtime::process::ProcessRuntime::new(manifest, root)
                        .map_err(|error| ExtError {
                            code: ferris_sdk::ErrorCode::Runtime,
                            message: error.to_string(),
                            details: serde_json::Value::Null,
                        })?
                        .with_config("{}".to_string())
                        .with_policy(Arc::clone(&self.options.policy), self.options.trust)
                        .with_workspace(self.options.workspace.clone())
                        .with_data_root(Some(data_root.to_path_buf()))
                        .with_events(
                            Arc::clone(&self.options.events),
                            &manifest.contributes.events,
                        );
                return Ok(Arc::new(
                    ferris_sdk::host_runtime::adapter::LegacyAdapter::new(
                        manifest.id().clone(),
                        root.to_path_buf(),
                        self.options.workspace.clone(),
                        Arc::new(runtime),
                    ),
                ));
            }
            if manifest.runtime.kind == RuntimeKind::Mcp && !self.options.process_sandbox {
                let runtime = ferris_sdk::host_runtime::mcp::McpRuntime::new(
                    manifest,
                    root,
                    "{}".to_string(),
                    Arc::clone(&self.options.policy),
                    self.options.trust,
                    Arc::clone(&self.options.events),
                )
                .map_err(|error| ExtError {
                    code: ferris_sdk::ErrorCode::Runtime,
                    message: error.to_string(),
                    details: serde_json::Value::Null,
                })?;
                return Ok(Arc::new(
                    ferris_sdk::host_runtime::adapter::LegacyAdapter::new(
                        manifest.id().clone(),
                        root.to_path_buf(),
                        self.options.workspace.clone(),
                        Arc::new(runtime),
                    ),
                ));
            }
            let ports = RuntimePorts {
                policy: Arc::clone(&self.options.policy),
                events: Arc::clone(&self.options.events),
                workspace: self.options.workspace.clone(),
                data_root: data_root.to_path_buf(),
                config: "{}".to_string(),
                trust: self.options.trust,
                terminal: Some(Arc::clone(&self.options.terminal)),
            };
            let built = build(manifest, root, ports).map_err(|error| ExtError {
                code: ferris_sdk::ErrorCode::Runtime,
                message: error.to_string(),
                details: serde_json::Value::Null,
            })?;
            Ok(Arc::new(
                ferris_sdk::host_runtime::adapter::LegacyAdapter::new(
                    manifest.id().clone(),
                    root.to_path_buf(),
                    self.options.workspace.clone(),
                    built.runtime,
                ),
            ))
        }
    }

    /// Host-side removal operations used by contribution leases.
    pub struct NoopLeaseHost;

    impl LeaseHost for NoopLeaseHost {
        fn remove_tools(&mut self, _owner: &str, _tools: &[String]) -> usize {
            0
        }
        fn remove_prompts(&mut self, _owner: &str, _sections: &[String]) -> usize {
            0
        }
        fn remove_skills(&mut self, _owner: &str, _paths: &[PathBuf]) -> usize {
            0
        }
        fn remove_registry_owner(&mut self, _owner: &str) -> usize {
            0
        }
    }

    /// Language callback pair delivered when contributions are applied/revoked.
    #[async_trait]
    pub trait ContributionCallback: Send + Sync {
        /// A runtime started with these contributions.
        async fn applied(
            &self,
            extension: &str,
            contributions: &[Contribution],
        ) -> Result<(), String>;
        /// A lease was revoked.
        async fn revoked(
            &self,
            extension: &str,
            contributions: &[Contribution],
        ) -> Result<(), String>;
    }

    struct AppliedEntry {
        token: u64,
        contributions: Vec<Contribution>,
    }

    /// The applied-contributions map shared by both bindings.
    #[derive(Default)]
    pub struct AppliedContributions {
        applied: StdMutex<BTreeMap<String, AppliedEntry>>,
        next_token: AtomicU64,
    }

    impl AppliedContributions {
        /// The applied map as a JSON object keyed by extension id.
        pub fn to_json(&self) -> serde_json::Value {
            let applied = self.applied.lock().ok();
            let map = applied
                .as_ref()
                .map(|applied| {
                    applied
                        .iter()
                        .map(|(extension, entry)| {
                            (
                                extension.clone(),
                                serde_json::to_value(&entry.contributions)
                                    .unwrap_or(serde_json::Value::Null),
                            )
                        })
                        .collect::<serde_json::Map<_, _>>()
                })
                .unwrap_or_default();
            serde_json::Value::Object(map)
        }
    }

    struct AppliedLease {
        book: Arc<AppliedContributions>,
        extension: String,
        token: u64,
        contributions: Vec<Contribution>,
        callback: Option<Arc<dyn ContributionCallback>>,
    }

    #[async_trait]
    impl ContributionLease for AppliedLease {
        async fn revoke(&self, _host: &mut dyn LeaseHost) -> Result<(), ExtError> {
            {
                let Ok(mut applied) = self.book.applied.lock() else {
                    return Ok(());
                };
                if applied.get(&self.extension).map(|entry| entry.token) == Some(self.token) {
                    applied.remove(&self.extension);
                }
            }
            if let Some(callback) = &self.callback {
                callback
                    .revoked(&self.extension, &self.contributions)
                    .await
                    .map_err(ferris_sdk::error::contribution)?;
            }
            Ok(())
        }
    }

    /// Record contributions and notify the host language.
    pub struct HostContributionSink {
        book: Arc<AppliedContributions>,
        callback: Option<Arc<dyn ContributionCallback>>,
    }

    impl HostContributionSink {
        /// Build a sink over the applied map and an optional callback.
        pub fn new(
            book: Arc<AppliedContributions>,
            callback: Option<Arc<dyn ContributionCallback>>,
        ) -> Self {
            Self { book, callback }
        }
    }

    #[async_trait]
    impl ContributionSink for HostContributionSink {
        async fn apply(
            &self,
            extension: &str,
            contributions: Vec<Contribution>,
            _agent: &mut (dyn Any + Send),
            _registry: &mut (dyn Any + Send),
        ) -> Result<Box<dyn ContributionLease>, ExtError> {
            if let Some(callback) = &self.callback {
                callback
                    .applied(extension, &contributions)
                    .await
                    .map_err(ferris_sdk::error::contribution)?;
            }
            let token = self.book.next_token.fetch_add(1, Ordering::Relaxed) + 1;
            if let Ok(mut applied) = self.book.applied.lock() {
                applied.insert(
                    extension.to_string(),
                    AppliedEntry {
                        token,
                        contributions: contributions.clone(),
                    },
                );
            }
            Ok(Box::new(AppliedLease {
                book: Arc::clone(&self.book),
                extension: extension.to_string(),
                token,
                contributions,
                callback: self.callback.clone(),
            }))
        }
    }

    /// A shell terminal for Cordis host calls.
    pub struct ShellTerminal;

    impl TerminalService for ShellTerminal {
        fn run(
            &self,
            command: &str,
            cwd: Option<PathBuf>,
        ) -> Result<ferris_sdk::terminal::TerminalOutput, String> {
            #[cfg(windows)]
            let mut process = {
                let mut process = std::process::Command::new("cmd");
                process.arg("/C").arg(command);
                process
            };
            #[cfg(not(windows))]
            let mut process = {
                let mut process = std::process::Command::new("sh");
                process.arg("-c").arg(command);
                process
            };
            if let Some(cwd) = cwd {
                process.current_dir(cwd);
            }
            let output = process.output().map_err(|error| error.to_string())?;
            Ok(ferris_sdk::terminal::TerminalOutput {
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                code: output.status.code().unwrap_or(-1),
            })
        }
    }

    /// A dedicated runtime for synchronous event-subscription methods.
    pub fn event_runtime() -> &'static tokio::runtime::Runtime {
        static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
        RUNTIME.get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .expect("event runtime")
        })
    }
}

#[cfg(feature = "host")]
pub use host::*;
