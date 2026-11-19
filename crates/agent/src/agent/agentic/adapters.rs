use super::*;

pub(super) struct ExtensibleToolAdapter {
    inner: Arc<dyn Tool + Send + Sync>,
}

impl ExtensibleToolAdapter {
    pub(super) fn new(
        inner: Arc<dyn Tool + Send + Sync>,
        _plugins: PluginRegistry,
        _hooks: HookRegistry,
        _agent_id: String,
    ) -> Self {
        Self { inner }
    }
}

impl ReactToolTrait for ExtensibleToolAdapter {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> String {
        self.inner.description()
    }

    fn json_schema(&self) -> serde_json::Value {
        self.inner.json_schema()
    }

    /// The wrapped tool's own declaration: a wrapper that dropped it would
    /// hide side effects from the retry policy and from discovery metadata.
    fn metadata(&self) -> Option<&react::tool::ToolMetadata> {
        self.inner.metadata()
    }

    fn run(&self, input: &serde_json::Value) -> Result<serde_json::Value, ReactToolError> {
        // Note: Hook triggering is handled by AgentReActApp at the agent level
        // to avoid duplicate hook firing. The tool's error is already the
        // canonical `ToolError`; re-wrapping it in `Failed(e.to_string())`
        // lost its class and added a "Tool execution failed:" prefix per
        // adapter layer.
        self.inner.run(input)
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

    /// Deferral and proxy identity are properties of the tool, not of this
    /// wrapper: a wrapper that answered them with its own defaults would
    /// advertise a withheld tool again and hide a dispatcher's target from the
    /// hooks.
    fn deferred(&self) -> bool {
        self.inner.deferred()
    }

    fn proxy_target(&self, args: &serde_json::Value) -> Option<(String, serde_json::Value)> {
        self.inner.proxy_target(args)
    }
}

pub(super) struct AsyncExtensibleToolAdapter {
    inner: Arc<dyn AsyncTool + Send + Sync>,
}

impl AsyncExtensibleToolAdapter {
    pub(super) fn new(inner: Arc<dyn AsyncTool + Send + Sync>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl AsyncTool for AsyncExtensibleToolAdapter {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> String {
        self.inner.description()
    }

    fn category(&self) -> String {
        "async".to_string()
    }

    async fn run(&self, input: &serde_json::Value) -> Result<serde_json::Value, ReactToolError> {
        // Preserve the canonical error. See [`ExtensibleToolAdapter::run`].
        self.inner.run(input).await
    }

    fn json_schema(&self) -> serde_json::Value {
        self.inner.json_schema()
    }

    /// See [`ExtensibleToolAdapter`]: the wrapper must not answer for the tool
    /// it wraps.
    fn metadata(&self) -> Option<&react::tool::ToolMetadata> {
        self.inner.metadata()
    }

    fn to_openai_definition(&self) -> react::tool::descriptor::ToolDefinition {
        self.inner.to_openai_definition()
    }

    fn is_skill(&self) -> bool {
        self.inner.is_skill()
    }

    fn is_cancelable(&self) -> bool {
        self.inner.is_cancelable()
    }

    /// See [`ExtensibleToolAdapter`]: the wrapper must not answer for the tool
    /// it wraps.
    fn deferred(&self) -> bool {
        self.inner.deferred()
    }

    /// See [`ExtensibleToolAdapter`].
    fn proxy_target(&self, args: &serde_json::Value) -> Option<(String, serde_json::Value)> {
        self.inner.proxy_target(args)
    }

    fn cancel(&self, call_id: &str) {
        self.inner.cancel(call_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tool that fails with a classified error.
    struct Failing(&'static str);

    impl Tool for Failing {
        fn name(&self) -> &str {
            self.0
        }

        fn description(&self) -> String {
            "Fails with a classified error".to_string()
        }

        fn json_schema(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }

        fn run(
            &self,
            _input: &serde_json::Value,
        ) -> Result<serde_json::Value, crate::tools::ToolError> {
            Err(crate::tools::ToolError::PermissionDenied {
                message: "the policy refused the write".to_string(),
            })
        }
    }

    /// The adapter used to flatten every failure into
    /// `Failed(e.to_string())`, which lost the class and stacked a
    /// "Tool execution failed:" prefix on every layer.
    #[test]
    fn the_adapter_preserves_the_tools_error_class() {
        let adapter = ExtensibleToolAdapter::new(
            Arc::new(Failing("failing")),
            PluginRegistry::default(),
            HookRegistry::default(),
            "agent".to_string(),
        );
        let error = adapter
            .run(&serde_json::json!({}))
            .expect_err("the tool fails");
        assert!(
            matches!(error, crate::tools::ToolError::PermissionDenied { .. }),
            "the class survives the adapter: {error:?}"
        );
        assert_eq!(
            error.to_string(),
            "Permission denied: the policy refused the write",
            "no duplicated prefix"
        );
    }
}
