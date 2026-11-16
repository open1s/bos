//! The first-party Rust runtime.
//!
//! A built-in extension is linked into BSH, so it does not need a dynamic ABI
//! or a process. It still has a manifest, still declares capabilities, and
//! still moves through the same lifecycle: being first-party is a trust level,
//! not an exemption from the contract.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::contract::{CallContext, Runtime, ToolDescriptor};
use ferris_core::manifest::Manifest;
use ferris_core::types::{ExtensionError, ExtensionId, RuntimeKind};

/// A first-party extension implemented in Rust.
#[async_trait]
pub trait BuiltinExtension: Send + Sync {
    /// The manifest, compiled into the binary.
    fn manifest_toml(&self) -> &'static str;

    /// The tools this extension offers. Every id must be declared in the
    /// manifest's contributes.tools.
    fn tools(&self) -> Vec<ToolDescriptor>;

    /// Run one tool by contribution id, with the call's own context.
    fn call_tool(
        &self,
        name: &str,
        arguments: &Value,
        context: &CallContext,
    ) -> Result<Value, ExtensionError>;

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

    /// The typed services this extension publishes (issue #302). Default none.
    /// Each must match a provides declaration in the manifest.
    fn provided_services(&self) -> Vec<crate::contract::ProvidedService> {
        Vec::new()
    }
}

/// The runtime wrapper around a BuiltinExtension.
pub struct BuiltinRuntime {
    extension: Arc<dyn BuiltinExtension>,
    manifest: Manifest,
    tools: Vec<ToolDescriptor>,
}

impl BuiltinRuntime {
    /// Validate and wrap one built-in extension.
    pub fn new(extension: Arc<dyn BuiltinExtension>) -> Result<Self, ExtensionError> {
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
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The extension id.
    pub fn id(&self) -> &ExtensionId {
        self.manifest.id()
    }

    /// The tool descriptors, without going through the async trait.
    pub fn descriptors(&self) -> Vec<ToolDescriptor> {
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
        context: &CallContext,
    ) -> Result<Value, ExtensionError> {
        if !self.tools.iter().any(|tool| tool.id.as_str() == name) {
            return Err(ExtensionError::NotFound {
                id: format!("{}::{name}", self.manifest.id()),
            });
        }
        self.extension.call_tool(name, &arguments, context)
    }

    fn provided_services(&self) -> Vec<crate::contract::ProvidedService> {
        self.extension.provided_services()
    }
}

/// A built-in extension defined by a closure.
///
/// This is the seam the host uses for a first-party contribution whose body
/// needs live host state. It is not a dynamic loader: the closure is Rust code
/// compiled into this binary, exactly like any other first-party extension.
pub struct ClosureExtension {
    manifest_toml: &'static str,
    tools: Vec<ToolDescriptor>,
    #[allow(clippy::type_complexity)]
    handler: Arc<dyn Fn(&str, &Value, &CallContext) -> Result<Value, ExtensionError> + Send + Sync>,
}

impl ClosureExtension {
    /// Build a closure-backed extension that does not read the call context.
    pub fn new(
        manifest_toml: &'static str,
        tools: Vec<ToolDescriptor>,
        handler: impl Fn(&str, &Value) -> Result<Value, ExtensionError> + Send + Sync + 'static,
    ) -> Self {
        Self::new_with_context(manifest_toml, tools, move |name, arguments, _context| {
            handler(name, arguments)
        })
    }

    /// Build a closure-backed extension that reads the call context.
    ///
    /// The context carries the calling session and workspace, which is what a
    /// tool acting on a turn needs; a two-argument handler cannot see either.
    pub fn new_with_context(
        manifest_toml: &'static str,
        tools: Vec<ToolDescriptor>,
        handler: impl Fn(&str, &Value, &CallContext) -> Result<Value, ExtensionError>
            + Send
            + Sync
            + 'static,
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

    fn call_tool(
        &self,
        name: &str,
        arguments: &Value,
        context: &CallContext,
    ) -> Result<Value, ExtensionError> {
        (self.handler)(name, arguments, context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::empty_schema;
    use ferris_core::types::ContributionId;

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

    /// A context-aware closure sees the calling session and workspace, which a
    /// two-argument handler cannot.
    #[tokio::test]
    async fn a_context_aware_closure_sees_the_session() {
        let runtime = BuiltinRuntime::new(Arc::new(ClosureExtension::new_with_context(
            MANIFEST,
            vec![ToolDescriptor {
                id: ContributionId::parse("echo").expect("id"),
                description: "Echo the context".to_string(),
                input_schema: empty_schema(),
                capabilities: Vec::new(),
            }],
            |_name, _arguments, context| {
                Ok(serde_json::json!({
                    "session": context.session,
                    "workspace": context
                        .workspace
                        .as_ref()
                        .map(|path| path.display().to_string()),
                }))
            },
        )))
        .expect("runtime");
        let context = CallContext::new(
            runtime.id().clone(),
            "correlation",
            Some(std::path::PathBuf::from("/tmp/ws")),
            1_000,
        )
        .with_session("ses-1");
        let result = runtime
            .call_tool("echo", serde_json::json!({}), &context)
            .await
            .expect("call");
        assert_eq!(result["session"], "ses-1");
        assert_eq!(result["workspace"], "/tmp/ws");
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

#[cfg(test)]
mod service_publication_tests {
    use super::*;
    use ferris_core::composable::{Scope, ServiceRegistry};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// The concrete service type the provider publishes and the consumer
    /// acquires back, typed.
    struct Counter {
        value: AtomicU64,
    }

    const PROVIDER_MANIFEST: &str = r#"
schema_version = "1.0"

[extension]
id = "com.example.provider"
name = "Provider"
version = "1.0.0"
description = "Publishes a counter service"
authors = ["Test"]
license = "MIT"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "builtin"
entry = "provider"
startup = "on-demand"

[[services.provides]]
name = "com.example.counter"
contract = "com.example.counter-api"
version = "1.0.0"
scope = "host"
"#;

    struct ProviderExt;

    impl BuiltinExtension for ProviderExt {
        fn manifest_toml(&self) -> &'static str {
            PROVIDER_MANIFEST
        }
        fn tools(&self) -> Vec<ToolDescriptor> {
            Vec::new()
        }
        fn call_tool(
            &self,
            _name: &str,
            _arguments: &Value,
            _context: &CallContext,
        ) -> Result<Value, ExtensionError> {
            Err(ExtensionError::UnsupportedRuntime {
                kind: "provider".to_string(),
            })
        }
        fn provided_services(&self) -> Vec<crate::contract::ProvidedService> {
            vec![crate::contract::ProvidedService::new(
                "com.example.counter",
                "com.example.counter-api",
                semver::Version::new(1, 0, 0),
                Scope::Host,
                Arc::new(Counter {
                    value: AtomicU64::new(7),
                }),
            )]
        }
    }

    /// Two independent extensions: a provider publishes a typed, versioned
    /// service at a shared scope and a consumer acquires it back as the same
    /// concrete type; unloading the provider revokes it (issue #302).
    #[test]
    fn a_provider_and_consumer_exchange_a_typed_service() {
        let provider = BuiltinRuntime::new(Arc::new(ProviderExt)).expect("provider runtime");
        let services = provider.provided_services();
        assert_eq!(services.len(), 1);

        // The host publishes the provider's service into the shared registry.
        let registry = ServiceRegistry::new();
        let token = services[0].publish(&registry);
        assert!(registry.has_name("com.example.counter"));

        // A different extension acquires it back, typed and version-checked.
        let requirement = semver::VersionReq::parse("^1").unwrap();
        let lease = registry
            .acquire_contract::<Counter>(
                &Scope::Host,
                "com.example.counter",
                "com.example.counter-api",
                &requirement,
            )
            .expect("the consumer acquires the typed service");
        assert_eq!(lease.value.load(Ordering::SeqCst), 7);
        drop(lease);

        // An incompatible version requirement is refused.
        let too_new = semver::VersionReq::parse("^2").unwrap();
        assert!(registry
            .acquire_contract::<Counter>(
                &Scope::Host,
                "com.example.counter",
                "com.example.counter-api",
                &too_new,
            )
            .is_none());

        // The provider unloads: its token revokes the service, so a consumer
        // can no longer acquire it (no stale lease).
        assert!(registry.revoke(&token));
        assert!(!registry.has_name("com.example.counter"));
        assert!(registry
            .acquire_contract::<Counter>(
                &Scope::Host,
                "com.example.counter",
                "com.example.counter-api",
                &requirement,
            )
            .is_none());
    }

    /// A runtime that provides no services returns none (the guest default).
    #[test]
    fn a_guest_runtime_provides_no_services() {
        let ext = ClosureExtension::new(PROVIDER_MANIFEST, Vec::new(), |_n, _a| Ok(Value::Null));
        let runtime = BuiltinRuntime::new(Arc::new(ext)).expect("runtime");
        assert!(runtime.provided_services().is_empty());
    }
}
