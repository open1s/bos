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

    fn run(&self, input: &serde_json::Value) -> Result<serde_json::Value, ReactToolError> {
        // Note: Hook triggering is handled by AgentReActApp at the agent level
        // to avoid duplicate hook firing
        self.inner
            .run(input)
            .map_err(|e| ReactToolError::Failed(e.to_string()))
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
        self.inner
            .run(input)
            .await
            .map_err(|e| ReactToolError::Failed(e.to_string()))
    }

    fn json_schema(&self) -> serde_json::Value {
        self.inner.json_schema()
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

    fn cancel(&self, call_id: &str) {
        self.inner.cancel(call_id);
    }
}
