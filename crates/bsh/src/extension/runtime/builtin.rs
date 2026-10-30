//! The first-party Rust runtime.
//!
//! A built-in extension is linked into BSH, so it does not need a dynamic ABI
//! or a process. It still has a manifest, still declares capabilities, and
//! still moves through the same lifecycle: being first-party is a trust level,
//! not an exemption from the contract.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use super::{CallContext, Runtime, ToolDescriptor};
use crate::extension::manifest::Manifest;
use crate::extension::types::{ExtensionError, ExtensionId, RuntimeKind};

/// A first-party extension implemented in Rust.
#[async_trait]
pub(crate) trait BuiltinExtension: Send + Sync {
    /// The manifest, compiled into the binary.
    fn manifest_toml(&self) -> &'static str;

    /// The tools this extension offers. Every id must be declared in the
    /// manifest's contributes.tools.
    fn tools(&self) -> Vec<ToolDescriptor>;

    /// Run one tool by contribution id.
    fn call_tool(&self, name: &str, arguments: &Value) -> Result<Value, ExtensionError>;

    /// Prepare the extension.
    async fn activate(&self) -> Result<(), ExtensionError> {
        Ok(())
    }

    /// Stop accepting calls.
    async fn deactivate(&self) -> Result<(), ExtensionError> {
        Ok(())
    }

    /// Release resources.
    async fn shutdown(&self) -> Result<(), ExtensionError> {
        Ok(())
    }
}

/// The runtime wrapper around a BuiltinExtension.
pub(crate) struct BuiltinRuntime {
    extension: Arc<dyn BuiltinExtension>,
    manifest: Manifest,
    tools: Vec<ToolDescriptor>,
}

impl BuiltinRuntime {
    /// Validate and wrap one built-in extension.
    pub(crate) fn new(extension: Arc<dyn BuiltinExtension>) -> Result<Self, ExtensionError> {
        let manifest = Manifest::parse(
            extension.manifest_toml(),
            std::path::Path::new("<builtin>/extension.toml"),
        )?;
        let report = manifest.validate();
        if !report.is_valid() {
            return Err(ExtensionError::Manifest {
                path: std::path::Path::new("<builtin>/extension.toml").to_path_buf(),
                message: report
                    .errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; "),
            });
        }
        if manifest.runtime.kind != RuntimeKind::Builtin {
            return Err(ExtensionError::Manifest {
                path: std::path::Path::new("<builtin>/extension.toml").to_path_buf(),
                message: "a built-in extension must declare runtime.kind = \"builtin\"".to_string(),
            });
        }
        let tools = extension.tools();
        let declared: std::collections::BTreeSet<&str> = manifest
            .contributes
            .tools
            .iter()
            .map(|id| id.as_str())
            .collect();
        for tool in &tools {
            if !declared.contains(tool.id.as_str()) {
                return Err(ExtensionError::NameConflict {
                    name: tool.id.as_str().to_string(),
                    owner: format!(
                        "built-in {} (not declared in contributes.tools)",
                        manifest.id()
                    ),
                });
            }
        }
        if declared.len() != tools.len() {
            return Err(ExtensionError::Manifest {
                path: std::path::Path::new("<builtin>/extension.toml").to_path_buf(),
                message: "every declared built-in tool must be offered by the implementation"
                    .to_string(),
            });
        }
        Ok(Self {
            extension,
            manifest,
            tools,
        })
    }

    /// The parsed and validated manifest.
    pub(crate) fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The extension id.
    pub(crate) fn id(&self) -> &ExtensionId {
        self.manifest.id()
    }

    /// The tool descriptors, without going through the async trait.
    pub(crate) fn descriptors(&self) -> Vec<ToolDescriptor> {
        self.tools.clone()
    }
}

#[async_trait]
impl Runtime for BuiltinRuntime {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::Builtin
    }

    async fn activate(&self) -> Result<(), ExtensionError> {
        self.extension.activate().await
    }

    async fn deactivate(&self) -> Result<(), ExtensionError> {
        self.extension.deactivate().await
    }

    async fn shutdown(&self) -> Result<(), ExtensionError> {
        self.extension.shutdown().await
    }

    async fn tools(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
        Ok(self.tools.clone())
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        _context: &CallContext,
    ) -> Result<Value, ExtensionError> {
        if !self.tools.iter().any(|tool| tool.id.as_str() == name) {
            return Err(ExtensionError::NotFound {
                id: format!("{}::{name}", self.manifest.id()),
            });
        }
        self.extension.call_tool(name, &arguments)
    }
}

/// A built-in extension defined by a closure.
///
/// This is the seam the host uses for a first-party contribution whose body
/// needs live host state. It is not a dynamic loader: the closure is Rust code
/// compiled into this binary, exactly like any other first-party extension.
pub(crate) struct ClosureExtension {
    manifest_toml: &'static str,
    tools: Vec<ToolDescriptor>,
    #[allow(clippy::type_complexity)]
    handler: Arc<dyn Fn(&str, &Value) -> Result<Value, ExtensionError> + Send + Sync>,
}

impl ClosureExtension {
    /// Build a closure-backed extension.
    pub(crate) fn new(
        manifest_toml: &'static str,
        tools: Vec<ToolDescriptor>,
        handler: impl Fn(&str, &Value) -> Result<Value, ExtensionError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            manifest_toml,
            tools,
            handler: Arc::new(handler),
        }
    }
}

#[async_trait]
impl BuiltinExtension for ClosureExtension {
    fn manifest_toml(&self) -> &'static str {
        self.manifest_toml
    }

    fn tools(&self) -> Vec<ToolDescriptor> {
        self.tools.clone()
    }

    fn call_tool(&self, name: &str, arguments: &Value) -> Result<Value, ExtensionError> {
        (self.handler)(name, arguments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::runtime::empty_schema;
    use crate::extension::types::ContributionId;

    const MANIFEST: &str = r#"
schema_version = "1.0"

[extension]
id = "com.bos.bsh.echo"
name = "Echo"
version = "1.0.0"
description = "A first-party echo tool"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "builtin"
entry = "echo"

[capabilities]
required = ["agent.tools.register"]

[contributes]
tools = ["echo"]
"#;

    fn extension() -> ClosureExtension {
        ClosureExtension::new(
            MANIFEST,
            vec![ToolDescriptor {
                id: ContributionId::parse("echo").expect("id"),
                description: "Echo the arguments".to_string(),
                input_schema: empty_schema(),
                capabilities: Vec::new(),
            }],
            |name, arguments| {
                if name != "echo" {
                    return Err(ExtensionError::NotFound {
                        id: name.to_string(),
                    });
                }
                Ok(arguments.clone())
            },
        )
    }

    #[tokio::test]
    async fn a_builtin_runs_through_the_runtime_contract() {
        let runtime = BuiltinRuntime::new(Arc::new(extension())).expect("runtime");
        assert_eq!(runtime.kind(), RuntimeKind::Builtin);
        let tools = runtime.tools().await.expect("tools");
        assert_eq!(tools.len(), 1);
        let context = CallContext::new(runtime.id().clone(), "correlation", None, 1000);
        let result = runtime
            .call_tool("echo", serde_json::json!({"x": 1}), &context)
            .await
            .expect("call");
        assert_eq!(result["x"], 1);
        assert!(runtime
            .call_tool("missing", serde_json::json!({}), &context)
            .await
            .is_err());
    }

    #[test]
    fn a_builtin_must_declare_every_tool_it_offers() {
        let extension = ClosureExtension::new(
            MANIFEST,
            vec![ToolDescriptor {
                id: ContributionId::parse("other").expect("id"),
                description: "Not declared".to_string(),
                input_schema: empty_schema(),
                capabilities: Vec::new(),
            }],
            |_, _| Ok(Value::Null),
        );
        assert!(BuiltinRuntime::new(Arc::new(extension)).is_err());
    }
}
