//! The Cordis / DeepSeek Harness runtime.
//!
//! A Cordis package's payload is a JavaScript apply(ctx, config) entry. This
//! runtime executes it in the existing sandboxed Node bridge, maps the
//! registrations it announces to BSH contributions, and proxies tool calls back
//! over the bridge protocol. The bridge is a subprocess: it does not gain
//! in-process access to the host.

use std::path::Path;
use std::sync::Mutex;

use serde_json::Value;

use agent::extensions::bridge::{
    find_interpreter, BridgeKind, BridgeOptions, BridgeRegistration, BridgeSession,
};

use super::{empty_schema, CallContext, Runtime, ToolDescriptor};
use crate::extension::manifest::Manifest;
use crate::extension::types::{ContributionId, ExtensionError, RuntimeKind};

/// A live Cordis plugin process and the registrations it announced.
pub(crate) struct CordisRuntime {
    extension: String,
    root: std::path::PathBuf,
    data_root: std::path::PathBuf,
    options: BridgeOptions,
    session: Mutex<Option<BridgeSession>>,
    registrations: Mutex<Vec<BridgeRegistration>>,
    tools: Mutex<Vec<ToolDescriptor>>,
}

impl CordisRuntime {
    /// Build the runtime, resolving Node and the package entry. The child is
    /// started lazily by activate() and tools().
    pub(crate) fn new(
        manifest: &Manifest,
        root: &Path,
        data_root: &Path,
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
            session: Mutex::new(None),
            registrations: Mutex::new(Vec::new()),
            tools: Mutex::new(Vec::new()),
        })
    }

    /// Start the bridge if needed and list the tools it registered.
    pub(crate) fn descriptors(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
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
        let mut session = self.session.lock().expect("cordis session lock");
        if session.is_some() {
            return Ok(());
        }
        let (started, manifest) = BridgeSession::start(self.options.clone())
            .map_err(|error| self.error(format!("{}: {error}", self.root.display())))?;
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
        self.shutdown().await
    }

    async fn shutdown(&self) -> Result<(), ExtensionError> {
        let session = self.session.lock().ok().and_then(|mut slot| slot.take());
        if let Some(session) = session {
            session.shutdown();
        }
        Ok(())
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
        let mut session = self.session.lock().expect("cordis session lock");
        let session = session
            .as_mut()
            .ok_or_else(|| self.error("the Cordis runtime is not running"))?;
        session
            .invoke(&registration, arguments)
            .map_err(|error| self.error(error))
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        _context: &CallContext,
    ) -> Result<Value, ExtensionError> {
        self.ensure()?;
        let registration = self
            .registration_id(name)
            .ok_or_else(|| self.error(format!("unknown Cordis tool {name}")))?;
        let mut session = self.session.lock().expect("cordis session lock");
        let session = session
            .as_mut()
            .ok_or_else(|| self.error("the Cordis runtime is not running"))?;
        session
            .invoke(&registration, arguments)
            .map_err(|error| self.error(error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::manifest::Manifest;
    use std::path::PathBuf;

    #[tokio::test]
    async fn a_real_cordis_entry_registers_and_calls_a_tool() {
        if find_interpreter().is_none() {
            return;
        }
        let root =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions/cordis-echo");
        let manifest = Manifest::load(&root).expect("manifest");
        let data_root =
            std::env::temp_dir().join(format!("bsh-cordis-test-{}-{}", std::process::id(), "echo"));
        let runtime = CordisRuntime::new(&manifest, &root, &data_root).expect("runtime");
        let tools = runtime.tools().await.expect("tools");
        assert!(tools.iter().any(|tool| tool.id.as_str() == "echo"));

        let context = CallContext::new(manifest.extension.id.clone(), "cordis-test", None, 5_000);
        let output = runtime
            .call_tool("echo", serde_json::json!({ "value": "hi" }), &context)
            .await
            .expect("call");
        assert_eq!(output["echoed"], "hi");
        runtime.shutdown().await.expect("shutdown");
        let _ = std::fs::remove_dir_all(&data_root);
    }
}
