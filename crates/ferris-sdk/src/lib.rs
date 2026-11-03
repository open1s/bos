#![warn(missing_docs)]
//! Author-facing SDK for the BOS extension framework.
//!
//! An extension implements the Extension trait and ships a 2.0 manifest.
//! There is no macro DSL: the manifest stays the single author entry point.

use ferris_core::{
    Contribution, ContributionRef, ExtError, InvokeContext, Runtime, RuntimeContext,
};
use serde_json::Value;

/// One invocation addressed to an extension.
#[derive(Debug, Clone)]
pub struct Call {
    /// Contribution being invoked.
    pub contribution: ContributionRef,
    /// Opaque input.
    pub input: Value,
}

/// Context available while an extension starts.
#[derive(Debug, Clone)]
pub struct ExtensionContext {
    /// Framework runtime context.
    pub runtime: RuntimeContext,
}

/// Author-facing extension interface.
#[async_trait::async_trait]
pub trait Extension: Send + Sync {
    /// The extension manifest, schema 2.0.
    fn manifest(&self) -> &'static str;

    /// Start and return contributions.
    async fn start(&self, ctx: &ExtensionContext) -> Result<Vec<Contribution>, ExtError>;

    /// Invoke one contribution.
    async fn invoke(&self, call: &Call, ctx: &InvokeContext) -> Result<Value, ExtError>;

    /// Shut down.
    async fn shutdown(&self) -> Result<(), ExtError> {
        Ok(())
    }
}

/// Typed builder for one contribution payload.
#[derive(Debug, Clone)]
pub struct ContributionBuilder {
    kind: String,
    id: String,
    payload: Value,
}

impl ContributionBuilder {
    /// Start a contribution of one host-registered kind.
    pub fn new(kind: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            id: id.into(),
            payload: Value::Null,
        }
    }

    /// Set the opaque payload.
    pub fn payload(mut self, payload: Value) -> Self {
        self.payload = payload;
        self
    }

    /// Build the contribution.
    pub fn build(self) -> Contribution {
        Contribution {
            kind: self.kind,
            id: self.id,
            payload: self.payload,
        }
    }
}

/// Adapt an author Extension into the low-level Runtime interface.
pub struct ExtensionRuntime<E> {
    extension: E,
    kind: String,
    root: std::path::PathBuf,
}

impl<E: Extension> ExtensionRuntime<E> {
    /// Wrap an extension as a runtime.
    pub fn new(extension: E, kind: impl Into<String>, root: std::path::PathBuf) -> Self {
        Self {
            extension,
            kind: kind.into(),
            root,
        }
    }
}

#[async_trait::async_trait]
impl<E: Extension> Runtime for ExtensionRuntime<E> {
    fn kind(&self) -> &str {
        &self.kind
    }

    async fn start(&self, ctx: &RuntimeContext) -> Result<Vec<Contribution>, ExtError> {
        if ctx.root != self.root {
            return Err(ExtError {
                code: ferris_core::ErrorCode::Runtime,
                message: "extension runtime root does not match its context".to_string(),
                details: Value::Null,
            });
        }
        self.extension
            .start(&ExtensionContext {
                runtime: ctx.clone(),
            })
            .await
    }

    async fn invoke(
        &self,
        contribution: &ContributionRef,
        input: Value,
        ctx: &InvokeContext,
    ) -> Result<Value, ExtError> {
        self.extension
            .invoke(
                &Call {
                    contribution: contribution.clone(),
                    input,
                },
                ctx,
            )
            .await
    }

    async fn shutdown(&self) -> Result<(), ExtError> {
        self.extension.shutdown().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferris_core::types::ExtensionId;

    struct Echo;

    #[async_trait::async_trait]
    impl Extension for Echo {
        fn manifest(&self) -> &'static str {
            "schema_version = \"2.0\""
        }

        async fn start(&self, _ctx: &ExtensionContext) -> Result<Vec<Contribution>, ExtError> {
            Ok(vec![ContributionBuilder::new("demo.echo", "echo").build()])
        }

        async fn invoke(&self, call: &Call, _ctx: &InvokeContext) -> Result<Value, ExtError> {
            Ok(call.input.clone())
        }
    }

    #[tokio::test]
    async fn the_runtime_adapter_starts_and_invokes() {
        let root = std::path::PathBuf::from("/tmp/demo");
        let runtime = ExtensionRuntime::new(Echo, "builtin", root.clone());
        let contributions = runtime
            .start(&RuntimeContext {
                extension: "com.example.demo".to_string(),
                root: root.clone(),
                data_root: root.join("data"),
            })
            .await
            .expect("start");
        assert_eq!(contributions.len(), 1);
        let value = runtime
            .invoke(
                &ContributionRef {
                    kind: "demo.echo".to_string(),
                    id: "echo".to_string(),
                },
                serde_json::json!({"ok": true}),
                &InvokeContext {
                    correlation: "c".to_string(),
                    timeout_ms: 100,
                },
            )
            .await
            .expect("invoke");
        assert_eq!(value, serde_json::json!({"ok": true}));
    }

    #[test]
    fn the_contribution_builder_is_typed() {
        let contribution = ContributionBuilder::new("demo.echo", "echo")
            .payload(serde_json::json!({"a": 1}))
            .build();
        assert_eq!(contribution.kind, "demo.echo");
        assert_eq!(contribution.id, "echo");
        let _ = ExtensionId::parse("com.example.demo").expect("id");
    }
}
