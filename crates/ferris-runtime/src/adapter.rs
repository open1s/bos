//! Bridge the legacy tool-shaped runtime contract to the generic runtime API.

use std::path::PathBuf;
use std::sync::Arc;

use ferris_core::types::ExtensionId;
use ferris_core::{Contribution, ContributionRef, ExtError, InvokeContext};

use crate::contract::{CallContext, Runtime as LegacyRuntime};

/// A generic runtime backed by one legacy BSH runtime.
pub struct LegacyAdapter {
    extension: ExtensionId,
    root: PathBuf,
    workspace: Option<PathBuf>,
    inner: Arc<dyn LegacyRuntime>,
}

impl LegacyAdapter {
    /// Wrap one legacy runtime.
    pub fn new(
        extension: ExtensionId,
        root: PathBuf,
        workspace: Option<PathBuf>,
        inner: Arc<dyn LegacyRuntime>,
    ) -> Self {
        Self {
            extension,
            root,
            workspace,
            inner,
        }
    }
}

fn runtime_error(error: impl std::fmt::Display) -> ExtError {
    ExtError {
        code: ferris_core::ErrorCode::Runtime,
        message: error.to_string(),
        details: serde_json::Value::Null,
    }
}

#[async_trait::async_trait]
impl ferris_core::Runtime for LegacyAdapter {
    fn kind(&self) -> &str {
        self.inner.kind().as_str()
    }

    async fn start(
        &self,
        _ctx: &ferris_core::RuntimeContext,
    ) -> Result<Vec<Contribution>, ExtError> {
        self.inner.activate().await.map_err(runtime_error)?;
        let tools = self.inner.tools().await.map_err(runtime_error)?;
        Ok(tools
            .into_iter()
            .map(|tool| Contribution {
                kind: "tool".to_string(),
                id: tool.id.as_str().to_string(),
                payload: serde_json::json!({
                    "description": tool.description,
                    "input_schema": tool.input_schema,
                    "capabilities": tool.capabilities,
                }),
            })
            .collect())
    }

    async fn invoke(
        &self,
        contribution: &ContributionRef,
        input: serde_json::Value,
        ctx: &InvokeContext,
    ) -> Result<serde_json::Value, ExtError> {
        if contribution.kind != "tool" {
            return Err(runtime_error(format!(
                "unsupported contribution kind {:?}",
                contribution.kind
            )));
        }
        let context = CallContext::new(
            self.extension.clone(),
            ctx.correlation.clone(),
            self.workspace.clone(),
            ctx.timeout_ms,
        );
        let result = self
            .inner
            .call_tool(&contribution.id, input, &context)
            .await
            .map_err(runtime_error)?;
        serde_json::to_value(result).map_err(runtime_error)
    }

    async fn shutdown(&self) -> Result<(), ExtError> {
        self.inner.shutdown().await.map_err(runtime_error)
    }
}

impl LegacyAdapter {
    /// The package root this adapter serves.
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::ToolDescriptor;

    struct Echo;

    #[async_trait::async_trait]
    impl LegacyRuntime for Echo {
        fn kind(&self) -> ferris_core::types::RuntimeKind {
            ferris_core::types::RuntimeKind::Declarative
        }

        async fn tools(&self) -> Result<Vec<ToolDescriptor>, ferris_core::types::ExtensionError> {
            Ok(vec![ToolDescriptor {
                id: ferris_core::types::ContributionId::parse("echo").expect("id"),
                description: "Echo".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
                capabilities: Vec::new(),
            }])
        }

        async fn call_tool(
            &self,
            _tool: &str,
            input: serde_json::Value,
            _context: &CallContext,
        ) -> Result<serde_json::Value, ferris_core::types::ExtensionError> {
            Ok(input)
        }
    }

    #[tokio::test]
    async fn exposes_tools_and_invokes_them() {
        let adapter = LegacyAdapter::new(
            ExtensionId::parse("com.example.echo").expect("extension"),
            std::path::PathBuf::from("/tmp"),
            None,
            Arc::new(Echo),
        );
        let starter = ferris_core::RuntimeContext {
            extension: "com.example.echo".to_string(),
            root: std::path::PathBuf::from("/tmp"),
            data_root: std::path::PathBuf::from("/tmp/data"),
        };
        let contributions = ferris_core::Runtime::start(&adapter, &starter)
            .await
            .expect("start");
        assert_eq!(contributions.len(), 1);
        let value = ferris_core::Runtime::invoke(
            &adapter,
            &ContributionRef {
                kind: "tool".to_string(),
                id: "echo".to_string(),
            },
            serde_json::json!({"hi": true}),
            &InvokeContext {
                correlation: "c".to_string(),
                timeout_ms: 1000,
            },
        )
        .await
        .expect("invoke");
        assert_eq!(value, serde_json::json!({"hi": true}));
    }
}
