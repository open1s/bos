//! Host-neutral construction of the standard runtimes.
//!
//! The host supplies ports and gets back the legacy runtime plus its tool and
//! declarative contribution projections. This keeps every concrete runtime
//! constructor out of the host adapter.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ferris_core::capability::PolicyEngine;
use ferris_core::events::EventBus;
use ferris_core::manifest::Manifest;
use ferris_core::terminal::TerminalService;
use ferris_core::types::{ExtensionError, RuntimeKind, StartupPolicy, TrustLevel};

use crate::declarative::{self, DeclarativeContributions};
use crate::workflow::WorkflowRuntime;
use crate::{LazyRuntime, Runtime, ToolDescriptor};

/// Host ports needed to build a standard runtime.
pub struct RuntimePorts {
    /// Authorization engine.
    pub policy: Arc<PolicyEngine>,
    /// Host event bus.
    pub events: Arc<EventBus>,
    /// Open workspace.
    pub workspace: Option<PathBuf>,
    /// Extension data root.
    pub data_root: PathBuf,
    /// Effective configuration JSON.
    pub config: String,
    /// Package trust.
    pub trust: TrustLevel,
    /// Terminal used by Cordis host calls.
    pub terminal: Option<Arc<dyn TerminalService>>,
}

/// One constructed runtime and its projections.
pub struct BuiltRuntime {
    /// Legacy runtime seam.
    pub runtime: Arc<dyn Runtime>,
    /// Tool descriptors.
    pub tools: Vec<ToolDescriptor>,
    /// Declarative contributions.
    pub contributions: DeclarativeContributions,
}

/// Build the runtime a manifest asks for.
pub fn build(
    manifest: &Manifest,
    root: &Path,
    ports: RuntimePorts,
) -> Result<BuiltRuntime, ExtensionError> {
    match manifest.runtime.kind {
        RuntimeKind::Declarative => {
            let runtime = declarative::DeclarativeRuntime::new(root, manifest)?;
            let tools = runtime.descriptors();
            let contributions = runtime.contributions().clone();
            finish(
                Arc::new(runtime),
                tools,
                contributions,
                &ports,
                manifest.resources.output_bytes(),
            )
        }
        RuntimeKind::Process => {
            let runtime = crate::process::ProcessRuntime::new(manifest, root)?
                .with_config(ports.config.clone())
                .with_policy(Arc::clone(&ports.policy), ports.trust)
                .with_workspace(ports.workspace.clone())
                .with_data_root(Some(ports.data_root.clone()))
                .with_events(Arc::clone(&ports.events), &manifest.contributes.events);
            let tools = runtime.descriptors();
            let contributions = declarative::load_contributions(root, manifest)?;
            finish(
                Arc::new(runtime),
                tools,
                contributions,
                &ports,
                manifest.resources.output_bytes(),
            )
        }
        RuntimeKind::Extism => {
            let runtime = crate::extism::ExtismRuntime::new(
                manifest,
                root,
                &ports.data_root,
                Arc::clone(&ports.policy),
                Arc::clone(&ports.events),
                ports.workspace.clone(),
                ports.config.clone(),
                ports.trust,
            )?;
            let tools = runtime.descriptors()?;
            let contributions = declarative::load_contributions(root, manifest)?;
            finish(
                Arc::new(runtime),
                tools,
                contributions,
                &ports,
                manifest.resources.output_bytes(),
            )
        }
        RuntimeKind::Mcp => {
            let runtime = crate::mcp::McpRuntime::new(
                manifest,
                root,
                ports.config.clone(),
                Arc::clone(&ports.policy),
                ports.trust,
                Arc::clone(&ports.events),
            )?;
            // An on-demand server is not started during host construction. Its
            // tools and contributions are discovered when the agent build
            // activates it, not when the store is merely scanned.
            let (tools, contributions) = if manifest.runtime.startup == StartupPolicy::OnDemand {
                (Vec::new(), DeclarativeContributions::default())
            } else {
                (runtime.descriptors()?, runtime.contributions()?)
            };
            finish(
                Arc::new(runtime),
                tools,
                contributions,
                &ports,
                manifest.resources.output_bytes(),
            )
        }
        RuntimeKind::WasmComponent => {
            let runtime = crate::wasm::WasmRuntime::new(
                manifest,
                root,
                &ports.data_root,
                Arc::clone(&ports.policy),
                Arc::clone(&ports.events),
                ports.workspace.clone(),
                ports.config.clone(),
                ports.trust,
            )?;
            let tools = runtime.descriptors()?;
            let contributions = declarative::load_contributions(root, manifest)?;
            finish(
                Arc::new(runtime),
                tools,
                contributions,
                &ports,
                manifest.resources.output_bytes(),
            )
        }
        RuntimeKind::Cordis => {
            let terminal = ports
                .terminal
                .clone()
                .ok_or_else(|| ExtensionError::Manifest {
                    path: root.join("extension.toml"),
                    message: "the Cordis runtime needs a terminal service".to_string(),
                })?;
            let runtime =
                crate::cordis::CordisRuntime::new(manifest, root, &ports.data_root, terminal)?
                    .with_authorization(Arc::clone(&ports.policy), ports.trust);
            let tools = runtime.descriptors()?;
            let contributions = declarative::load_contributions(root, manifest)?;
            finish(
                Arc::new(runtime),
                tools,
                contributions,
                &ports,
                manifest.resources.output_bytes(),
            )
        }
        RuntimeKind::Builtin => Err(ExtensionError::Manifest {
            path: root.join("extension.toml"),
            message: "the builtin runtime is linked in; an installed package cannot claim it"
                .to_string(),
        }),
    }
}

fn finish(
    runtime: Arc<dyn Runtime>,
    tools: Vec<ToolDescriptor>,
    contributions: DeclarativeContributions,
    ports: &RuntimePorts,
    max_output_bytes: u64,
) -> Result<BuiltRuntime, ExtensionError> {
    let runtime: Arc<dyn Runtime> = Arc::new(LazyRuntime::new(runtime));
    if contributions.workflows.is_empty() {
        return Ok(BuiltRuntime {
            runtime,
            tools,
            contributions,
        });
    }
    let wrapped = WorkflowRuntime::new(
        runtime,
        &contributions.workflows,
        &tools,
        Arc::clone(&ports.policy),
        ports.trust,
        max_output_bytes,
    )?;
    let tools = wrapped.descriptors();
    Ok(BuiltRuntime {
        runtime: Arc::new(LazyRuntime::new(Arc::new(wrapped))),
        tools,
        contributions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferris_core::capability::PolicyMode;
    use ferris_core::events::EventBus;

    fn ports(root: &Path) -> RuntimePorts {
        let temp = std::env::temp_dir().join(format!("ferris-factory-{}", std::process::id()));
        std::fs::create_dir_all(&temp).expect("temp");
        RuntimePorts {
            policy: Arc::new(
                PolicyEngine::open(
                    temp.join("grants.toml"),
                    temp.join("audit.log"),
                    PolicyMode::Enforce,
                )
                .expect("policy"),
            ),
            events: EventBus::new(),
            workspace: None,
            data_root: root.join("data"),
            config: "{}".to_string(),
            trust: TrustLevel::Community,
            terminal: None,
        }
    }

    #[test]
    fn builds_a_declarative_runtime() {
        let root =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions/hello");
        let manifest = ferris_core::manifest_v2::load_package_manifest(&root).expect("manifest");
        let built = build(&manifest, &root, ports(&root)).expect("factory");
        assert!(built.tools.iter().any(|tool| tool.id.as_str() == "echo"));
    }

    #[test]
    fn builds_a_process_runtime_without_spawning() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/extensions/process-echo");
        let manifest = ferris_core::manifest_v2::load_package_manifest(&root).expect("manifest");
        let built = build(&manifest, &root, ports(&root)).expect("factory");
        assert!(built.tools.iter().any(|tool| tool.id.as_str() == "echo"));
    }

    #[tokio::test]
    async fn an_on_demand_mcp_package_is_not_discovered_by_the_factory() {
        if std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let root =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions/mcp-echo");
        let manifest = ferris_core::manifest_v2::load_package_manifest(&root).expect("manifest");
        assert_eq!(manifest.runtime.startup, StartupPolicy::OnDemand);
        let ports = ports(&root);
        ports
            .policy
            .grant(
                "com.example.mcp-echo",
                ferris_core::capability::Capability::ProcessExecute,
                ferris_core::capability::GrantScope::Program("python3".to_string()),
                None,
            )
            .expect("grant");
        let built = build(&manifest, &root, ports).expect("factory");
        assert!(
            built.tools.is_empty(),
            "the factory must not start an on-demand MCP server"
        );
        let tools = built.runtime.tools().await.expect("activation discovery");
        assert!(tools.iter().any(|tool| tool.id.as_str() == "echo"));
    }
}
