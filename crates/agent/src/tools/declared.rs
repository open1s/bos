//! Declaring capability metadata over an existing tool.
//!
//! A host composes built-in tools it did not write, so the declaration belongs
//! to the composition site rather than to the tool type. The wrapper changes
//! nothing about execution: every call, cancellation, deferral, and proxy
//! answer is the inner tool's. It only adds the static metadata a catalogue
//! needs, which is what makes "who may offer this capability, and at what risk"
//! answerable without reading the body.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use react::tool::registry::{AsyncTool, Tool};
use react::tool::{ToolDefinition, ToolError, ToolMetadata};
use serde_json::Value;

/// A synchronous tool with declared capability metadata.
pub struct DeclaredTool {
    inner: Arc<dyn Tool>,
    metadata: ToolMetadata,
}

impl DeclaredTool {
    /// Wrap `inner` with the metadata the host declares for it.
    pub fn new(inner: Arc<dyn Tool>, metadata: ToolMetadata) -> Self {
        Self { inner, metadata }
    }

    /// The tool this wrapper delegates to.
    pub fn inner(&self) -> &Arc<dyn Tool> {
        &self.inner
    }
}

impl Tool for DeclaredTool {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> String {
        self.inner.description()
    }

    fn category(&self) -> String {
        self.inner.category()
    }

    fn run(&self, input: &Value) -> Result<Value, ToolError> {
        self.inner.run(input)
    }

    fn json_schema(&self) -> Value {
        self.inner.json_schema()
    }

    fn to_openai_definition(&self) -> ToolDefinition {
        self.inner.to_openai_definition()
    }

    fn is_skill(&self) -> bool {
        self.inner.is_skill()
    }

    fn is_cancelable(&self) -> bool {
        self.inner.is_cancelable()
    }

    fn cancel(&self, call_id: &str) {
        self.inner.cancel(call_id);
    }

    fn deferred(&self) -> bool {
        self.inner.deferred()
    }

    fn proxy_target(&self, args: &Value) -> Option<(String, Value)> {
        self.inner.proxy_target(args)
    }

    fn metadata(&self) -> Option<&ToolMetadata> {
        Some(&self.metadata)
    }
}

/// An asynchronous tool with declared capability metadata.
pub struct DeclaredAsyncTool {
    inner: Arc<dyn AsyncTool>,
    metadata: ToolMetadata,
}

impl DeclaredAsyncTool {
    /// Wrap `inner` with the metadata the host declares for it.
    pub fn new(inner: Arc<dyn AsyncTool>, metadata: ToolMetadata) -> Self {
        Self { inner, metadata }
    }

    /// The tool this wrapper delegates to.
    pub fn inner(&self) -> &Arc<dyn AsyncTool> {
        &self.inner
    }
}

#[async_trait]
impl AsyncTool for DeclaredAsyncTool {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> String {
        self.inner.description()
    }

    fn category(&self) -> String {
        self.inner.category()
    }

    async fn run(&self, input: &Value) -> Result<Value, ToolError> {
        self.inner.run(input).await
    }

    async fn run_with_call_id(&self, input: &Value, call_id: &str) -> Result<Value, ToolError> {
        self.inner.run_with_call_id(input, call_id).await
    }

    fn json_schema(&self) -> Value {
        self.inner.json_schema()
    }

    fn to_openai_definition(&self) -> ToolDefinition {
        self.inner.to_openai_definition()
    }

    fn is_skill(&self) -> bool {
        self.inner.is_skill()
    }

    fn supports_streaming(&self) -> bool {
        self.inner.supports_streaming()
    }

    fn is_cancelable(&self) -> bool {
        self.inner.is_cancelable()
    }

    fn cancel(&self, call_id: &str) {
        self.inner.cancel(call_id);
    }

    fn deferred(&self) -> bool {
        self.inner.deferred()
    }

    fn proxy_target(&self, args: &Value) -> Option<(String, Value)> {
        self.inner.proxy_target(args)
    }

    fn metadata(&self) -> Option<&ToolMetadata> {
        Some(&self.metadata)
    }

    async fn run_streaming(
        &self,
        input: &Value,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String, ToolError>> + Send>>, ToolError> {
        self.inner.run_streaming(input).await
    }

    async fn run_streaming_with_call_id(
        &self,
        input: &Value,
        call_id: &str,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String, ToolError>> + Send>>, ToolError> {
        self.inner.run_streaming_with_call_id(input, call_id).await
    }
}
