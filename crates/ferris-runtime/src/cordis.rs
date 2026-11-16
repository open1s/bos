//! The Cordis / DeepSeek Harness runtime.
//!
//! A Cordis package's payload is a JavaScript apply(ctx, config) entry. This
//! runtime executes it in the existing capability-gated Node bridge, maps the
//! registrations it announces to BSH contributions, and proxies tool calls back
//! over the bridge protocol. The bridge is a subprocess: it does not gain
//! in-process access to the host.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::bridge::{
    find_interpreter, BridgeKind, BridgeOptions, BridgeRegistration, BridgeSession,
};

use crate::contract::{empty_schema, CallContext, Runtime, ToolDescriptor};
use ferris_core::capability::{authorize_each, CallScope, Capability, PolicyEngine};
use ferris_core::manifest::Manifest;
use ferris_core::types::{ContributionId, ExtensionError, RuntimeKind, TrustLevel};

/// A live Cordis plugin process and the registrations it announced.
pub struct CordisRuntime {
    extension: String,
    root: std::path::PathBuf,
    data_root: std::path::PathBuf,
    options: BridgeOptions,
    session: Arc<Mutex<Option<BridgeSession>>>,
    /// Terminal shutdown; deactivate remains reversible.
    stopped: AtomicBool,
    registrations: Mutex<Vec<BridgeRegistration>>,
    tools: Mutex<Vec<ToolDescriptor>>,
    terminal: Arc<dyn ferris_core::terminal::TerminalService>,
    /// Policy used for host service calls. Absent means deny.
    policy: Option<Arc<PolicyEngine>>,
    trust: TrustLevel,
    /// The open workspace, handed to tool and panel calls as call metadata.
    workspace: Option<std::path::PathBuf>,
}

impl CordisRuntime {
    /// Build the runtime, resolving Node and the package entry. The child is
    /// started lazily by activate() and tools().
    pub fn new(
        manifest: &Manifest,
        root: &Path,
        data_root: &Path,
        terminal: Arc<dyn ferris_core::terminal::TerminalService>,
    ) -> Result<Self, ExtensionError> {
        let extension = manifest.extension.id.as_str().to_owned();
        let interpreter = find_interpreter().ok_or_else(|| ExtensionError::UnsupportedRuntime {
            kind: format!("cordis (no Node interpreter for {extension})"),
        })?;
        let entry = manifest
            .runtime
            .entry
            .clone()
            .ok_or_else(|| ExtensionError::Runtime {
                extension: extension.clone(),
                message: "the Cordis runtime needs an entry".to_string(),
            })?;
        let config = manifest
            .configuration
            .as_ref()
            .and_then(|configuration| configuration.defaults.as_ref())
            .map(|defaults| serde_json::to_value(defaults).unwrap_or(Value::Null))
            .unwrap_or(Value::Null);
        let options =
            BridgeOptions::new(interpreter, root.to_path_buf(), entry).with_config(config);
        Ok(Self {
            extension,
            root: root.to_path_buf(),
            data_root: data_root.to_path_buf(),
            options,
            session: Arc::new(Mutex::new(None)),
            stopped: AtomicBool::new(false),
            registrations: Mutex::new(Vec::new()),
            tools: Mutex::new(Vec::new()),
            terminal,
            policy: None,
            trust: TrustLevel::Community,
            workspace: None,
        })
    }

    /// Attach the policy used to authorize bridge host calls such as
    /// terminal.run. Without it those calls are denied.
    #[must_use]
    pub fn with_authorization(mut self, policy: Arc<PolicyEngine>, trust: TrustLevel) -> Self {
        self.policy = Some(policy);
        self.trust = trust;
        self
    }

    /// Attach the workspace the package's tools and panels operate on.
    #[must_use]
    pub fn with_workspace(mut self, workspace: Option<std::path::PathBuf>) -> Self {
        self.workspace = workspace;
        self
    }

    /// Run the synchronous bridge exchange on a blocking worker and apply the
    /// caller's deadline to the bridge read.
    async fn invoke_async(
        &self,
        registration: String,
        arguments: Value,
        timeout_ms: u64,
        workspace: Option<std::path::PathBuf>,
    ) -> Result<Value, ExtensionError> {
        let session = Arc::clone(&self.session);
        let outcome = tokio::task::spawn_blocking(move || {
            let mut slot = session
                .lock()
                .map_err(|_| "cordis session lock is poisoned".to_string())?;
            let session = slot
                .as_mut()
                .ok_or_else(|| "the Cordis runtime is not running".to_string())?;
            session
                .invoke_with_timeout(
                    &registration,
                    arguments,
                    Some(std::time::Duration::from_millis(timeout_ms.max(1))),
                    workspace.as_deref(),
                )
                .map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| self.error(format!("cordis invoke task failed: {error}")))?;
        outcome.map_err(|error| self.error(error))
    }

    /// Take and terminate the current bridge process. A poisoned mutex still
    /// owns a live child, so recover it rather than treating the slot as empty.
    fn stop_session(&self) -> Result<(), ExtensionError> {
        let mut slot = match self.session.lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        let session = slot.take();
        drop(slot);
        if let Some(session) = session {
            session.shutdown();
        }
        Ok(())
    }

    /// Start the bridge if needed and list the tools it registered.
    pub fn descriptors(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
        self.ensure()?;
        Ok(self
            .tools
            .lock()
            .map(|tools| tools.clone())
            .unwrap_or_default())
    }

    fn error(&self, message: impl std::fmt::Display) -> ExtensionError {
        ExtensionError::Runtime {
            extension: self.extension.clone(),
            message: message.to_string(),
        }
    }

    fn ensure(&self) -> Result<(), ExtensionError> {
        if self.stopped.load(Ordering::SeqCst) {
            return Err(ExtensionError::Runtime {
                extension: self.extension.clone(),
                message: "runtime is shut down".to_string(),
            });
        }
        let mut session = self.session.lock().expect("cordis session lock");
        if session.is_some() {
            return Ok(());
        }
        let (mut started, manifest) = BridgeSession::start(self.options.clone())
            .map_err(|error| self.error(format!("{}: {error}", self.root.display())))?;
        let terminal = Arc::clone(&self.terminal);
        let policy = self.policy.clone();
        let trust = self.trust;
        let owner = self.extension.clone();
        let workspace = self.root.clone();
        started.set_host_call(Arc::new(
            move |service: &str, method: &str, params: Value| {
                if service != "terminal" || method != "run" {
                    return Err(format!("unsupported host service {service}.{method}"));
                }
                let command = params
                    .get("command")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "terminal.run needs a command".to_string())?;
                let cwd = params
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(std::path::PathBuf::from);
                let policy = policy
                    .as_ref()
                    .ok_or_else(|| "terminal.run is not authorized".to_string())?;
                let scope = CallScope {
                    workspace: Some(workspace.clone()),
                    path: Some(cwd.clone().unwrap_or_else(|| workspace.clone())),
                    program: Some(command.to_string()),
                    ..CallScope::default()
                };
                authorize_each(policy, &owner, trust, &[Capability::ProcessExecute], &scope)
                    .map_err(|error| error.to_string())?;
                let output = terminal.run(command, cwd)?;
                Ok(serde_json::json!({
                    "stdout": output.stdout,
                    "stderr": output.stderr,
                    "code": output.code,
                }))
            },
        ));

        let mut tools = Vec::new();
        for registration in &manifest.registrations {
            if registration.kind != BridgeKind::Tool {
                continue;
            }
            let id = ContributionId::parse(&registration.name)
                .map_err(|error| self.error(format!("tool {}: {error}", registration.name)))?;
            tools.push(ToolDescriptor {
                id,
                description: registration.description.clone(),
                input_schema: registration.schema.clone().unwrap_or_else(empty_schema),
                capabilities: Vec::new(),
            });
        }
        let skill_root = self.data_root.join("skills");
        for registration in &manifest.registrations {
            if registration.kind != BridgeKind::Skill {
                continue;
            }
            let Some(body) = &registration.content else {
                continue;
            };
            let directory = skill_root.join(&registration.name);
            std::fs::create_dir_all(&directory)
                .map_err(|error| self.error(format!("skill {}: {error}", registration.name)))?;
            let document = format!(
                "---\nname: {}\ndescription: {}\n---\n\n{}",
                registration.name, registration.description, body
            );
            std::fs::write(directory.join("SKILL.md"), document)
                .map_err(|error| self.error(format!("skill {}: {error}", registration.name)))?;
        }
        // A hook registration must name a stage this host actually dispatches;
        // refuse it at load rather than exposing a listener nothing will call
        // (issue #300).
        for registration in &manifest.registrations {
            if registration.kind != BridgeKind::Hook {
                continue;
            }
            for stage in &registration.stages {
                if !cordis_stage_is_dispatchable(stage) {
                    return Err(self.error(format!(
                        "hook {} claims stage {stage}, which this host does not dispatch",
                        registration.name
                    )));
                }
            }
        }
        *self.registrations.lock().expect("cordis registration lock") = manifest.registrations;
        *self.tools.lock().expect("cordis tools lock") = tools;
        *session = Some(started);
        Ok(())
    }

    fn registration_id(&self, name: &str) -> Option<String> {
        self.registrations
            .lock()
            .ok()?
            .iter()
            .find(|registration| {
                registration.kind == BridgeKind::Tool
                    && (registration.name == name || registration.id == name)
            })
            .map(|registration| registration.id.clone())
    }
}

#[async_trait::async_trait]
impl Runtime for CordisRuntime {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::Cordis
    }

    fn skills_dir(&self) -> Option<std::path::PathBuf> {
        Some(self.data_root.join("skills"))
    }

    async fn activate(&self) -> Result<(), ExtensionError> {
        self.ensure()
    }

    async fn deactivate(&self) -> Result<(), ExtensionError> {
        self.stop_session()
    }

    async fn shutdown(&self) -> Result<(), ExtensionError> {
        self.stopped.store(true, Ordering::SeqCst);
        self.stop_session()
    }

    async fn tools(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
        self.ensure()?;
        Ok(self
            .tools
            .lock()
            .map(|tools| tools.clone())
            .unwrap_or_default())
    }

    async fn command(&self, id: &str, arguments: Value) -> Result<Value, ExtensionError> {
        self.ensure()?;
        let registration = self
            .registrations
            .lock()
            .ok()
            .and_then(|registrations| {
                registrations
                    .iter()
                    .find(|registration| {
                        registration.kind == BridgeKind::Command
                            && (registration.name == id || registration.id == id)
                    })
                    .map(|registration| registration.id.clone())
            })
            .ok_or_else(|| self.error(format!("unknown Cordis command {id}")))?;
        let timeout = self.options.invoke_timeout.as_millis() as u64;
        self.invoke_async(registration, arguments, timeout, None)
            .await
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        context: &CallContext,
    ) -> Result<Value, ExtensionError> {
        self.ensure()?;
        let registration = self
            .registration_id(name)
            .ok_or_else(|| self.error(format!("unknown Cordis tool {name}")))?;
        self.invoke_async(
            registration,
            arguments,
            context.timeout_ms,
            context.workspace.clone(),
        )
        .await
    }

    /// Serve one panel's live card from the ctx.panels registration named by
    /// the panel's contribution id — the Cordis analog of the wasm runtime's
    /// call_panel export. Panel actions stay tool calls, so one engine answers
    /// both the first frame and every later press.
    async fn panel(&self, id: &str) -> Result<Value, ExtensionError> {
        self.ensure()?;
        let registration = self
            .registrations
            .lock()
            .ok()
            .and_then(|registrations| {
                registrations
                    .iter()
                    .find(|registration| {
                        registration.kind == BridgeKind::Panel
                            && (registration.name == id || registration.id == id)
                    })
                    .map(|registration| registration.id.clone())
            })
            .ok_or_else(|| self.error(format!("unknown Cordis panel {id}")))?;
        let timeout = self.options.invoke_timeout.as_millis() as u64;
        self.invoke_async(
            registration,
            serde_json::json!({}),
            timeout,
            self.workspace.clone(),
        )
        .await
    }

    /// Deliver one observe hook batch to the Cordis listeners that mapped onto
    /// this BSH hook point (issue #300).
    ///
    /// The canonical mapping is cordis_hook_stage: a BSH point with no Cordis
    /// event has no listeners and is a no-op, never a false success. Matching
    /// registrations are invoked in registration order, each bounded by the
    /// invoke timeout; a listener error propagates to the hook worker, which
    /// logs it rather than failing the turn.
    async fn hook(&self, point: &str, payload: Value) -> Result<(), ExtensionError> {
        let Some(stage) = cordis_hook_stage(point) else {
            return Ok(());
        };
        self.ensure()?;
        let listeners: Vec<String> = self
            .registrations
            .lock()
            .ok()
            .map(|registrations| {
                registrations
                    .iter()
                    .filter(|registration| {
                        registration.kind == BridgeKind::Hook
                            && registration.stages.iter().any(|s| s == stage)
                    })
                    .map(|registration| registration.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        let timeout = self.options.invoke_timeout.as_millis() as u64;
        for registration in listeners {
            self.invoke_async(registration, payload.clone(), timeout, None)
                .await?;
        }
        Ok(())
    }
}

/// The canonical mapping from a BSH hook point to the Cordis stage the shim
/// registers (issue #300).
///
/// Only points with a real Cordis event map; the rest return None so a hook
/// the bridge cannot dispatch is never reported as active. The shim's
/// HOOK_EVENTS table is the other half of this contract.
pub fn cordis_hook_stage(point: &str) -> Option<&'static str> {
    match point {
        "model.request" => Some("llm_request"),
        "model.response" => Some("llm_response"),
        "tool.before" => Some("tool_call"),
        "tool.after" => Some("tool_result"),
        _ => None,
    }
}

/// Whether a Cordis stage names a BSH hook point this host dispatches.
///
/// Used at load to refuse a hook registration that claims a stage nothing will
/// ever call, rather than exposing a successful registration.
pub fn cordis_stage_is_dispatchable(stage: &str) -> bool {
    matches!(
        stage,
        "llm_request" | "llm_response" | "stream_token" | "tool_call" | "tool_result"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct TestTerminal;

    impl ferris_core::terminal::TerminalService for TestTerminal {
        fn run(
            &self,
            command: &str,
            _cwd: Option<std::path::PathBuf>,
        ) -> Result<ferris_core::terminal::TerminalOutput, String> {
            let output = std::process::Command::new("sh")
                .arg("-c")
                .arg(command)
                .output()
                .map_err(|error| error.to_string())?;
            Ok(ferris_core::terminal::TerminalOutput {
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                code: output.status.code().unwrap_or(-1),
            })
        }
    }

    // Runs on every host: the load frame carries an ordinary drive path (the
    // verbatim \? prefix is stripped), which Node resolves and imports
    // through pathToFileURL (issue #304).
    #[tokio::test]
    async fn a_real_cordis_entry_registers_and_calls_a_tool() {
        if find_interpreter().is_none() {
            return;
        }
        let root =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions/cordis-echo");
        let manifest = ferris_core::manifest_v2::load_package_manifest(&root).expect("manifest");
        let data_root =
            std::env::temp_dir().join(format!("bsh-cordis-test-{}-{}", std::process::id(), "echo"));
        std::fs::create_dir_all(&data_root).expect("data root");
        let policy = Arc::new(
            PolicyEngine::open(
                data_root.join("grants.toml"),
                data_root.join("audit.jsonl"),
                ferris_core::capability::PolicyMode::Enforce,
            )
            .expect("policy"),
        );
        policy
            .grant(
                manifest.id().as_str(),
                Capability::ProcessExecute,
                ferris_core::capability::GrantScope::Any,
                None,
            )
            .expect("grant");
        let runtime = CordisRuntime::new(&manifest, &root, &data_root, Arc::new(TestTerminal))
            .expect("runtime")
            .with_authorization(policy, TrustLevel::Local);
        let tools = runtime.tools().await.expect("tools");
        assert!(tools.iter().any(|tool| tool.id.as_str() == "echo"));

        let context = CallContext::new(manifest.extension.id.clone(), "cordis-test", None, 5_000);
        let output = runtime
            .call_tool("echo", serde_json::json!({ "value": "hi" }), &context)
            .await
            .expect("call");
        assert_eq!(output["echoed"], "hi");
        assert_eq!(output["terminal"], "bridge-ok");
        runtime.shutdown().await.expect("shutdown");
        let _ = std::fs::remove_dir_all(&data_root);
    }

    /// A ctx.panels registration serves the live card, and the call's
    /// workspace reaches both the panel render and a tool's execute as the
    /// second argument — the seam a workspace-acting panel needs.
    #[tokio::test]
    async fn a_panel_registration_serves_the_live_card_with_the_workspace() {
        if find_interpreter().is_none() {
            return;
        }
        let root =
            std::env::temp_dir().join(format!("bsh-cordis-panel-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("root");
        std::fs::write(
            root.join("extension.toml"),
            r#"
schema_version = "2.0"
[extension]
id = "com.example.cordis-panel"
name = "Cordis Panel"
version = "1.0.0"
description = "A panel seam test"
[compatibility]
host = ">=3.0.0, <4.0.0"
extension_api = "^1.0"
[runtime]
kind = "cordis"
[runtime.config]
entry = "index.js"
[capabilities]
required = ["agent.tools.register", "ui.panels"]
[[contribution]]
kind = "tool"
id = "echo_ws"
[contribution.payload]
[[contribution]]
kind = "panel"
id = "card"
[contribution.payload]
"#,
        )
        .expect("manifest");
        std::fs::write(
            root.join("index.js"),
            r#"
module.exports = {
  apply(ctx) {
    ctx.panels.register({
      name: 'card',
      description: 'test card',
      render(args, meta) {
        return {
          schema_version: '1.0',
          root: { type: 'text', text: 'ws=' + (meta && meta.workspace) },
        };
      },
    });
    ctx.tools.register({
      name: 'echo_ws',
      description: 'echo the call workspace',
      parameters: { type: 'object', properties: {} },
      async execute(args, meta) {
        return { workspace: meta && meta.workspace };
      },
    });
  },
};
"#,
        )
        .expect("entry");
        let manifest = ferris_core::manifest_v2::load_package_manifest(&root).expect("manifest");
        let data_root = root.join("data");
        let workspace = root.join("workspace");
        let runtime = CordisRuntime::new(&manifest, &root, &data_root, Arc::new(TestTerminal))
            .expect("runtime")
            .with_workspace(Some(workspace.clone()));

        let card = runtime.panel("card").await.expect("panel card");
        assert_eq!(card["root"]["text"], format!("ws={}", workspace.display()));
        assert!(runtime.panel("missing").await.is_err());

        let context = CallContext::new(
            manifest.extension.id.clone(),
            "cordis-panel-test",
            Some(workspace.clone()),
            5_000,
        );
        let output = runtime
            .call_tool("echo_ws", serde_json::json!({}), &context)
            .await
            .expect("call");
        assert_eq!(output["workspace"], workspace.display().to_string());

        runtime.shutdown().await.expect("shutdown");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The canonical BSH-point to Cordis-stage mapping (issue #300).
    #[test]
    fn the_hook_mapping_covers_the_dispatchable_points() {
        assert_eq!(cordis_hook_stage("model.request"), Some("llm_request"));
        assert_eq!(cordis_hook_stage("model.response"), Some("llm_response"));
        assert_eq!(cordis_hook_stage("tool.before"), Some("tool_call"));
        assert_eq!(cordis_hook_stage("tool.after"), Some("tool_result"));
        // Points with no Cordis event map to nothing.
        assert_eq!(cordis_hook_stage("turn.start"), None);
        assert_eq!(cordis_hook_stage("http.exchange"), None);
        assert_eq!(cordis_hook_stage("skill.load"), None);
    }

    /// Every mapped stage is dispatchable, and an unknown stage is not.
    #[test]
    fn only_real_stages_are_dispatchable() {
        for point in [
            "model.request",
            "model.response",
            "tool.before",
            "tool.after",
        ] {
            let stage = cordis_hook_stage(point).unwrap();
            assert!(cordis_stage_is_dispatchable(stage));
        }
        assert!(cordis_stage_is_dispatchable("stream_token"));
        assert!(!cordis_stage_is_dispatchable("made_up_event"));
    }
}
