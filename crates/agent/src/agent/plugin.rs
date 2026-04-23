use async_trait::async_trait;
use futures::FutureExt;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LlmStage {
    PreRequest,
    PostResponse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolStage {
    PreExecute,
    PostExecute,
}

#[derive(Debug, Clone)]
pub struct LlmRequestWrapper {
    pub model: String,
    pub context: react::llm::LlmContext,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub top_p: Option<f32>,
    pub top_k: Option<u32>,
    pub metadata: std::collections::HashMap<String, String>,
}

impl LlmRequestWrapper {
    pub fn new(request: &react::llm::LlmRequest) -> Self {
        Self {
            model: request.model.clone(),
            context: request.context.clone(),
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            top_p: request.top_p,
            top_k: request.top_k,
            metadata: std::collections::HashMap::new(),
        }
    }

    pub fn into_request(self) -> react::llm::LlmRequest {
        react::llm::LlmRequest {
            model: self.model,
            context: self.context,
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            top_p: self.top_p,
            top_k: self.top_k,
        }
    }
}

#[derive(Debug, Clone)]
pub enum LlmResponseWrapper {
    Text(String),
    Partial(String),
    ToolCall {
        name: String,
        args: serde_json::Value,
        id: Option<String>,
    },
    Done,
}

#[derive(Debug, Clone)]
pub enum StreamTokenWrapper {
    Text(String),
    ToolCall {
        name: String,
        args: serde_json::Value,
        id: Option<String>,
    },
    Done,
}

impl StreamTokenWrapper {
    pub fn new(token: &react::llm::StreamToken) -> Self {
        match token {
            react::llm::StreamToken::Text(s) => StreamTokenWrapper::Text(s.clone()),
            react::llm::StreamToken::ToolCall { name, args, id } => StreamTokenWrapper::ToolCall {
                name: name.clone(),
                args: args.clone(),
                id: id.clone(),
            },
            react::llm::StreamToken::Done => StreamTokenWrapper::Done,
        }
    }

    pub fn into_token(self) -> react::llm::StreamToken {
        match self {
            StreamTokenWrapper::Text(s) => react::llm::StreamToken::Text(s),
            StreamTokenWrapper::ToolCall { name, args, id } => {
                react::llm::StreamToken::ToolCall { name, args, id }
            }
            StreamTokenWrapper::Done => react::llm::StreamToken::Done,
        }
    }
}

impl LlmResponseWrapper {
    pub fn new(response: &react::llm::LlmResponse) -> Self {
        match response {
            react::llm::LlmResponse::Text(s) => LlmResponseWrapper::Text(s.clone()),
            react::llm::LlmResponse::Partial(s) => LlmResponseWrapper::Partial(s.clone()),
            react::llm::LlmResponse::Done => LlmResponseWrapper::Done,
            react::llm::LlmResponse::ToolCall { name, args, id } => LlmResponseWrapper::ToolCall {
                name: name.clone(),
                args: args.clone(),
                id: id.clone(),
            },
        }
    }

    pub fn into_response(self) -> react::llm::LlmResponse {
        match self {
            LlmResponseWrapper::Text(s) => react::llm::LlmResponse::Text(s),
            LlmResponseWrapper::Partial(s) => react::llm::LlmResponse::Partial(s),
            LlmResponseWrapper::Done => react::llm::LlmResponse::Done,
            LlmResponseWrapper::ToolCall { name, args, id } => {
                react::llm::LlmResponse::ToolCall { name, args, id }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolCallWrapper {
    pub name: String,
    pub args: serde_json::Value,
    pub id: Option<String>,
    pub metadata: std::collections::HashMap<String, String>,
}

impl ToolCallWrapper {
    pub fn new(name: impl Into<String>, args: serde_json::Value, id: Option<String>) -> Self {
        Self {
            name: name.into(),
            args,
            id,
            metadata: std::collections::HashMap::new(),
        }
    }

    pub fn from_tool_call(name: &str, args: &serde_json::Value, id: Option<&str>) -> Self {
        Self {
            name: name.to_string(),
            args: args.clone(),
            id: id.map(|s| s.to_string()),
            metadata: std::collections::HashMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolResultWrapper {
    pub result: serde_json::Value,
    pub success: bool,
    pub error: Option<String>,
    pub metadata: std::collections::HashMap<String, String>,
}

impl ToolResultWrapper {
    pub fn new(result: serde_json::Value) -> Self {
        Self {
            result,
            success: true,
            error: None,
            metadata: std::collections::HashMap::new(),
        }
    }

    pub fn from_result(result: &Result<serde_json::Value, react::tool::ToolError>) -> Self {
        match result {
            Ok(v) => Self::new(v.clone()),
            Err(e) => Self {
                result: serde_json::Value::Null,
                success: false,
                error: Some(e.to_string()),
                metadata: std::collections::HashMap::new(),
            },
        }
    }

    pub fn into_result(self) -> Result<serde_json::Value, react::tool::ToolError> {
        if self.success {
            Ok(self.result)
        } else {
            Err(react::tool::ToolError::Failed(
                self.error.unwrap_or_else(|| "Unknown error".to_string()),
            ))
        }
    }
}

#[async_trait]
pub trait AgentPlugin: Send + Sync + 'static {
    fn name(&self) -> &str;

    async fn on_llm_request(&self, request: LlmRequestWrapper) -> Option<LlmRequestWrapper> {
        let _ = request;
        None
    }

    async fn on_llm_response(&self, response: LlmResponseWrapper) -> Option<LlmResponseWrapper> {
        let _ = response;
        None
    }

    async fn on_tool_call(&self, tool_call: ToolCallWrapper) -> Option<ToolCallWrapper> {
        let _ = tool_call;
        None
    }

    async fn on_tool_result(&self, tool_result: ToolResultWrapper) -> Option<ToolResultWrapper> {
        let _ = tool_result;
        None
    }

    async fn on_stream_token(&self, token: StreamTokenWrapper) -> Option<StreamTokenWrapper> {
        let _ = token;
        None
    }
}

#[derive(Default, Clone)]
pub struct PluginRegistry {
    plugins: Arc<RwLock<Vec<Arc<dyn AgentPlugin>>>>,
    plugin_count: Arc<AtomicUsize>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn register(&self, plugin: Arc<dyn AgentPlugin>) {
        let mut plugins = self.plugins.write().await;
        plugins.push(plugin);
        self.plugin_count.store(plugins.len(), Ordering::Release);
    }

    pub async fn plugins(&self) -> Vec<Arc<dyn AgentPlugin>> {
        self.plugins.read().await.clone()
    }

    pub fn plugins_blocking(&self) -> Vec<Arc<dyn AgentPlugin>> {
        block_on_future(self.plugins())
    }

    pub fn plugin_names_blocking(&self) -> Vec<String> {
        self.plugins_blocking()
            .iter()
            .map(|p| p.name().to_string())
            .collect()
    }

    pub fn register_blocking(&self, plugin: Arc<dyn AgentPlugin>) {
        block_on_future(self.register(plugin));
    }

    pub fn len(&self) -> usize {
        self.plugin_count.load(Ordering::Acquire)
    }

    pub fn has_plugins(&self) -> bool {
        self.len() > 0
    }

    pub async fn process_llm_request(&self, mut request: LlmRequestWrapper) -> LlmRequestWrapper {
        let plugins = self.plugins().await;
        for plugin in plugins {
            let plugin_name = plugin.name().to_string();
            let result = AssertUnwindSafe(plugin.on_llm_request(request.clone()))
                .catch_unwind()
                .await;
            match result {
                Ok(Some(modified)) => {
                    request = modified;
                }
                Ok(None) => {}
                Err(_) => {
                    log::warn!(
                        "Plugin '{}' panicked during on_llm_request; skipping",
                        plugin_name
                    );
                }
            }
        }
        request
    }

    pub fn process_llm_request_blocking(&self, request: LlmRequestWrapper) -> LlmRequestWrapper {
        block_on_future(self.process_llm_request(request))
    }

    pub async fn process_llm_response(
        &self,
        mut response: LlmResponseWrapper,
    ) -> LlmResponseWrapper {
        let plugins = self.plugins().await;
        for plugin in plugins {
            let plugin_name = plugin.name().to_string();
            let result = AssertUnwindSafe(plugin.on_llm_response(response.clone()))
                .catch_unwind()
                .await;
            match result {
                Ok(Some(modified)) => {
                    response = modified;
                }
                Ok(None) => {}
                Err(_) => {
                    log::warn!(
                        "Plugin '{}' panicked during on_llm_response; skipping",
                        plugin_name
                    );
                }
            }
        }
        response
    }

    pub fn process_llm_response_blocking(
        &self,
        response: LlmResponseWrapper,
    ) -> LlmResponseWrapper {
        block_on_future(self.process_llm_response(response))
    }

    pub async fn process_stream_token(&self, mut token: StreamTokenWrapper) -> StreamTokenWrapper {
        let plugins = self.plugins().await;
        for plugin in plugins {
            let plugin_name = plugin.name().to_string();
            let result = AssertUnwindSafe(plugin.on_stream_token(token.clone()))
                .catch_unwind()
                .await;
            match result {
                Ok(Some(modified)) => {
                    token = modified;
                }
                Ok(None) => {}
                Err(_) => {
                    log::warn!(
                        "Plugin '{}' panicked during on_stream_token; skipping",
                        plugin_name
                    );
                }
            }
        }
        token
    }

    pub fn process_stream_token_blocking(&self, token: StreamTokenWrapper) -> StreamTokenWrapper {
        block_on_future(self.process_stream_token(token))
    }

    pub async fn process_tool_call(&self, mut tool_call: ToolCallWrapper) -> ToolCallWrapper {
        let plugins = self.plugins().await;
        for plugin in plugins {
            let plugin_name = plugin.name().to_string();
            let result = AssertUnwindSafe(plugin.on_tool_call(tool_call.clone()))
                .catch_unwind()
                .await;
            match result {
                Ok(Some(modified)) => {
                    tool_call = modified;
                }
                Ok(None) => {}
                Err(_) => {
                    log::warn!(
                        "Plugin '{}' panicked during on_tool_call; skipping",
                        plugin_name
                    );
                }
            }
        }
        tool_call
    }

    pub fn process_tool_call_blocking(&self, tool_call: ToolCallWrapper) -> ToolCallWrapper {
        block_on_future(self.process_tool_call(tool_call))
    }

    pub async fn process_tool_result(
        &self,
        mut tool_result: ToolResultWrapper,
    ) -> ToolResultWrapper {
        let plugins = self.plugins().await;
        for plugin in plugins {
            let plugin_name = plugin.name().to_string();
            let result = AssertUnwindSafe(plugin.on_tool_result(tool_result.clone()))
                .catch_unwind()
                .await;
            match result {
                Ok(Some(modified)) => {
                    tool_result = modified;
                }
                Ok(None) => {}
                Err(_) => {
                    log::warn!(
                        "Plugin '{}' panicked during on_tool_result; skipping",
                        plugin_name
                    );
                }
            }
        }
        tool_result
    }

    pub fn process_tool_result_blocking(
        &self,
        tool_result: ToolResultWrapper,
    ) -> ToolResultWrapper {
        block_on_future(self.process_tool_result(tool_result))
    }

    pub async fn clear(&self) {
        let mut plugins = self.plugins.write().await;
        plugins.clear();
        self.plugin_count.store(0, Ordering::Release);
    }

    pub fn clear_blocking(&self) {
        block_on_future(self.clear());
    }
}

fn block_on_future<F: Future>(future: F) -> F::Output {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        if matches!(
            handle.runtime_flavor(),
            tokio::runtime::RuntimeFlavor::MultiThread
        ) {
            tokio::task::block_in_place(|| handle.block_on(future))
        } else {
            futures::executor::block_on(future)
        }
    } else {
        futures::executor::block_on(future)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct TestPlugin {
        name: String,
    }

    impl TestPlugin {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
            }
        }
    }

    #[async_trait]
    impl AgentPlugin for TestPlugin {
        fn name(&self) -> &str {
            &self.name
        }

        async fn on_llm_request(
            &self,
            mut request: LlmRequestWrapper,
        ) -> Option<LlmRequestWrapper> {
            request
                .metadata
                .insert("plugin".to_string(), self.name.clone());
            Some(request)
        }

        async fn on_tool_call(&self, mut tool_call: ToolCallWrapper) -> Option<ToolCallWrapper> {
            tool_call
                .metadata
                .insert("plugin".to_string(), self.name.clone());
            Some(tool_call)
        }
    }

    #[tokio::test]
    async fn test_plugin_registry_register() {
        let registry = PluginRegistry::new();
        let plugin = Arc::new(TestPlugin::new("test"));

        registry.register(plugin.clone()).await;

        let plugins = registry.plugins().await;
        assert_eq!(plugins.len(), 1);
    }

    #[test]
    fn test_plugin_registry_register_blocking() {
        let registry = PluginRegistry::new();
        registry.register_blocking(Arc::new(TestPlugin::new("blocking")));
        assert!(registry.has_plugins());
        assert_eq!(registry.len(), 1);
    }

    #[tokio::test]
    async fn test_plugin_registry_process_llm_request() {
        let registry = PluginRegistry::new();
        let plugin = Arc::new(TestPlugin::new("modifier"));

        registry.register(plugin).await;

        let request = react::llm::LlmRequest {
            model: "gpt-4".to_string(),
            context: Default::default(),
            ..Default::default()
        };
        let wrapper = LlmRequestWrapper::new(&request);

        let result = registry.process_llm_request(wrapper).await;
        assert_eq!(result.metadata.get("plugin"), Some(&"modifier".to_string()));
    }

    #[tokio::test]
    async fn test_plugin_registry_process_tool_call() {
        let registry = PluginRegistry::new();
        let plugin = Arc::new(TestPlugin::new("tool-modifier"));

        registry.register(plugin).await;

        let tool_call = ToolCallWrapper::new("add", serde_json::json!({"a": 1, "b": 2}), None);

        let result = registry.process_tool_call(tool_call).await;
        assert_eq!(
            result.metadata.get("plugin"),
            Some(&"tool-modifier".to_string())
        );
    }

    #[tokio::test]
    async fn test_plugin_registry_multiple_plugins() {
        let registry = PluginRegistry::new();

        registry
            .register(Arc::new(TestPlugin::new("plugin1")))
            .await;
        registry
            .register(Arc::new(TestPlugin::new("plugin2")))
            .await;

        let plugins = registry.plugins().await;
        assert_eq!(plugins.len(), 2);
    }

    #[tokio::test]
    async fn test_llm_request_wrapper() {
        let request = react::llm::LlmRequest {
            model: "gpt-4".to_string(),
            context: Default::default(),
            temperature: Some(0.5),
            max_tokens: Some(100),
            ..Default::default()
        };

        let wrapper = LlmRequestWrapper::new(&request);
        assert_eq!(wrapper.model, "gpt-4");
        assert_eq!(wrapper.temperature, Some(0.5));
        assert_eq!(wrapper.max_tokens, Some(100));

        let back = wrapper.into_request();
        assert_eq!(back.model, "gpt-4");
    }

    #[tokio::test]
    async fn test_tool_result_wrapper() {
        let result = Ok(serde_json::json!({"status": "ok"}));
        let wrapper = ToolResultWrapper::from_result(&result);
        assert!(wrapper.success);
        assert!(wrapper.error.is_none());

        let back = wrapper.into_result().unwrap();
        assert_eq!(back, serde_json::json!({"status": "ok"}));

        let err_result: Result<serde_json::Value, _> =
            Err(react::tool::ToolError::Failed("test error".to_string()));
        let err_wrapper = ToolResultWrapper::from_result(&err_result);
        assert!(!err_wrapper.success);
        assert!(err_wrapper.error.is_some());
    }

    #[tokio::test]
    async fn test_llm_response_wrapper_text() {
        let response = react::llm::LlmResponse::Text("hello".to_string());
        let wrapper = LlmResponseWrapper::new(&response);
        match wrapper {
            LlmResponseWrapper::Text(s) => assert_eq!(s, "hello"),
            _ => panic!("expected Text"),
        }
    }

    #[tokio::test]
    async fn test_llm_response_wrapper_tool_call() {
        let response = react::llm::LlmResponse::ToolCall {
            name: "add".to_string(),
            args: serde_json::json!({"a": 1, "b": 2}),
            id: Some("call_123".to_string()),
        };
        let wrapper = LlmResponseWrapper::new(&response);
        match wrapper {
            LlmResponseWrapper::ToolCall { name, args: _, id } => {
                assert_eq!(name, "add");
                assert_eq!(id, Some("call_123".to_string()));
            }
            _ => panic!("expected ToolCall"),
        }
    }

    #[tokio::test]
    async fn test_llm_response_wrapper_done() {
        let response = react::llm::LlmResponse::Done;
        let wrapper = LlmResponseWrapper::new(&response);
        match wrapper {
            LlmResponseWrapper::Done => {}
            _ => panic!("expected Done"),
        }
    }

    #[tokio::test]
    async fn test_llm_response_wrapper_into_response() {
        let wrapper = LlmResponseWrapper::Text("response".to_string());
        let response = wrapper.into_response();
        assert!(matches!(response, react::llm::LlmResponse::Text(s) if s == "response"));
    }

    #[tokio::test]
    async fn test_stream_token_wrapper_text() {
        let token = react::llm::StreamToken::Text("hello".to_string());
        let wrapper = StreamTokenWrapper::new(&token);
        match wrapper {
            StreamTokenWrapper::Text(s) => assert_eq!(s, "hello"),
            _ => panic!("expected Text"),
        }
    }

    #[tokio::test]
    async fn test_stream_token_wrapper_tool_call() {
        let token = react::llm::StreamToken::ToolCall {
            name: "add".to_string(),
            args: serde_json::json!({"a": 1}),
            id: Some("call_456".to_string()),
        };
        let wrapper = StreamTokenWrapper::new(&token);
        match wrapper {
            StreamTokenWrapper::ToolCall { name, args: _, id } => {
                assert_eq!(name, "add");
                assert_eq!(id, Some("call_456".to_string()));
            }
            _ => panic!("expected ToolCall"),
        }
    }

    #[tokio::test]
    async fn test_stream_token_wrapper_done() {
        let token = react::llm::StreamToken::Done;
        let wrapper = StreamTokenWrapper::new(&token);
        match wrapper {
            StreamTokenWrapper::Done => {}
            _ => panic!("expected Done"),
        }
    }

    #[tokio::test]
    async fn test_stream_token_wrapper_into_token() {
        let wrapper = StreamTokenWrapper::Text("stream text".to_string());
        let token = wrapper.into_token();
        assert!(matches!(token, react::llm::StreamToken::Text(s) if s == "stream text"));
    }

    #[tokio::test]
    async fn test_tool_call_wrapper_new() {
        let wrapper = ToolCallWrapper::new(
            "multiply",
            serde_json::json!({"x": 5, "y": 10}),
            Some("call_789".to_string()),
        );
        assert_eq!(wrapper.name, "multiply");
        assert_eq!(wrapper.id, Some("call_789".to_string()));
    }

    #[tokio::test]
    async fn test_tool_call_wrapper_from_tool_call() {
        let args = serde_json::json!({"a": 1, "b": 2});
        let wrapper = ToolCallWrapper::from_tool_call("divide", &args, Some("call_div"));
        assert_eq!(wrapper.name, "divide");
        assert_eq!(wrapper.args, args);
        assert_eq!(wrapper.id, Some("call_div".to_string()));
    }

    #[tokio::test]
    async fn test_plugin_modifies_model() {
        #[derive(Debug)]
        struct ModelSwitcher;
        #[async_trait]
        impl AgentPlugin for ModelSwitcher {
            fn name(&self) -> &str {
                "model-switcher"
            }
            async fn on_llm_request(
                &self,
                mut request: LlmRequestWrapper,
            ) -> Option<LlmRequestWrapper> {
                request.model = "gpt-3.5-turbo".to_string();
                Some(request)
            }
        }

        let registry = PluginRegistry::new();
        registry.register(Arc::new(ModelSwitcher)).await;

        let request = react::llm::LlmRequest {
            model: "gpt-4".to_string(),
            context: Default::default(),
            ..Default::default()
        };
        let wrapper = LlmRequestWrapper::new(&request);
        let result = registry.process_llm_request(wrapper).await;
        assert_eq!(result.model, "gpt-3.5-turbo");
    }

    #[tokio::test]
    async fn test_plugin_modifies_temperature() {
        #[derive(Debug)]
        struct TempSetter;
        #[async_trait]
        impl AgentPlugin for TempSetter {
            fn name(&self) -> &str {
                "temp-setter"
            }
            async fn on_llm_request(
                &self,
                mut request: LlmRequestWrapper,
            ) -> Option<LlmRequestWrapper> {
                request.temperature = Some(1.5);
                Some(request)
            }
        }

        let registry = PluginRegistry::new();
        registry.register(Arc::new(TempSetter)).await;

        let request = react::llm::LlmRequest {
            model: "gpt-4".to_string(),
            context: Default::default(),
            temperature: None,
            ..Default::default()
        };
        let wrapper = LlmRequestWrapper::new(&request);
        let result = registry.process_llm_request(wrapper).await;
        assert_eq!(result.temperature, Some(1.5));
    }

    #[tokio::test]
    async fn test_plugin_chain_multiple_plugins() {
        #[derive(Debug)]
        struct PrefixPlugin;
        #[async_trait]
        impl AgentPlugin for PrefixPlugin {
            fn name(&self) -> &str {
                "prefix"
            }
            async fn on_llm_request(
                &self,
                mut request: LlmRequestWrapper,
            ) -> Option<LlmRequestWrapper> {
                request
                    .metadata
                    .insert("prefix".to_string(), "added-by-prefix".to_string());
                Some(request)
            }
        }

        #[derive(Debug)]
        struct SuffixPlugin;
        #[async_trait]
        impl AgentPlugin for SuffixPlugin {
            fn name(&self) -> &str {
                "suffix"
            }
            async fn on_llm_request(
                &self,
                mut request: LlmRequestWrapper,
            ) -> Option<LlmRequestWrapper> {
                request
                    .metadata
                    .insert("suffix".to_string(), "added-by-suffix".to_string());
                Some(request)
            }
        }

        let registry = PluginRegistry::new();
        registry.register(Arc::new(PrefixPlugin)).await;
        registry.register(Arc::new(SuffixPlugin)).await;

        let request = react::llm::LlmRequest {
            model: "gpt-4".to_string(),
            context: Default::default(),
            ..Default::default()
        };
        let wrapper = LlmRequestWrapper::new(&request);
        let result = registry.process_llm_request(wrapper).await;

        assert_eq!(
            result.metadata.get("prefix"),
            Some(&"added-by-prefix".to_string())
        );
        assert_eq!(
            result.metadata.get("suffix"),
            Some(&"added-by-suffix".to_string())
        );
    }

    #[tokio::test]
    async fn test_plugin_on_llm_response_modifies() {
        #[derive(Debug)]
        struct ResponseModifier;
        #[async_trait]
        impl AgentPlugin for ResponseModifier {
            fn name(&self) -> &str {
                "response-modifier"
            }
            async fn on_llm_response(
                &self,
                response: LlmResponseWrapper,
            ) -> Option<LlmResponseWrapper> {
                if let LlmResponseWrapper::Text(s) = response {
                    return Some(LlmResponseWrapper::Text(s.to_uppercase()));
                }
                None
            }
        }

        let registry = PluginRegistry::new();
        registry.register(Arc::new(ResponseModifier)).await;

        let wrapper = LlmResponseWrapper::Text("hello world".to_string());
        let result = registry.process_llm_response(wrapper).await;
        match result {
            LlmResponseWrapper::Text(s) => assert_eq!(s, "HELLO WORLD"),
            _ => panic!("expected modified Text"),
        }
    }

    #[tokio::test]
    async fn test_plugin_on_tool_call_modifies_args() {
        #[derive(Debug)]
        struct ArgsModifier;
        #[async_trait]
        impl AgentPlugin for ArgsModifier {
            fn name(&self) -> &str {
                "args-modifier"
            }
            async fn on_tool_call(
                &self,
                mut tool_call: ToolCallWrapper,
            ) -> Option<ToolCallWrapper> {
                if tool_call.name == "add" {
                    tool_call.args = serde_json::json!({"a": 100, "b": 200});
                }
                Some(tool_call)
            }
        }

        let registry = PluginRegistry::new();
        registry.register(Arc::new(ArgsModifier)).await;

        let tool_call = ToolCallWrapper::new("add", serde_json::json!({"a": 1, "b": 2}), None);
        let result = registry.process_tool_call(tool_call).await;

        assert_eq!(result.args, serde_json::json!({"a": 100, "b": 200}));
    }

    #[tokio::test]
    async fn test_plugin_on_tool_result_modifies() {
        #[derive(Debug)]
        struct ResultModifier;
        #[async_trait]
        impl AgentPlugin for ResultModifier {
            fn name(&self) -> &str {
                "result-modifier"
            }
            async fn on_tool_result(
                &self,
                mut tool_result: ToolResultWrapper,
            ) -> Option<ToolResultWrapper> {
                if tool_result.success {
                    tool_result
                        .metadata
                        .insert("modified".to_string(), "true".to_string());
                }
                Some(tool_result)
            }
        }

        let registry = PluginRegistry::new();
        registry.register(Arc::new(ResultModifier)).await;

        let tool_result = ToolResultWrapper::new(serde_json::json!({"status": "ok"}));
        let result = registry.process_tool_result(tool_result).await;

        assert!(result.success);
        assert_eq!(result.metadata.get("modified"), Some(&"true".to_string()));
    }

    #[tokio::test]
    async fn test_plugin_returns_none_no_modification() {
        #[derive(Debug)]
        struct NoopPlugin;
        #[async_trait]
        impl AgentPlugin for NoopPlugin {
            fn name(&self) -> &str {
                "noop"
            }
            async fn on_llm_request(
                &self,
                _request: LlmRequestWrapper,
            ) -> Option<LlmRequestWrapper> {
                None
            }
        }

        let registry = PluginRegistry::new();
        registry.register(Arc::new(NoopPlugin)).await;

        let request = react::llm::LlmRequest {
            model: "gpt-4".to_string(),
            context: Default::default(),
            ..Default::default()
        };
        let wrapper = LlmRequestWrapper::new(&request);
        let result = registry.process_llm_request(wrapper).await;

        assert_eq!(result.model, "gpt-4");
        assert!(result.metadata.is_empty());
    }

    #[tokio::test]
    async fn test_plugin_registry_clear() {
        let registry = PluginRegistry::new();
        registry
            .register(Arc::new(TestPlugin::new("plugin1")))
            .await;
        registry
            .register(Arc::new(TestPlugin::new("plugin2")))
            .await;

        let before = registry.plugins().await;
        assert_eq!(before.len(), 2);

        registry.clear().await;

        let after = registry.plugins().await;
        assert_eq!(after.len(), 0);
        assert!(!registry.has_plugins());
        assert_eq!(registry.len(), 0);
    }

    #[tokio::test]
    async fn test_tool_result_wrapper_metadata() {
        let mut wrapper = ToolResultWrapper::new(serde_json::json!({"result": "ok"}));
        wrapper
            .metadata
            .insert("trace_id".to_string(), "abc123".to_string());

        assert_eq!(
            wrapper.metadata.get("trace_id"),
            Some(&"abc123".to_string())
        );

        let back = wrapper.into_result().unwrap();
        assert_eq!(back, serde_json::json!({"result": "ok"}));
    }

    #[tokio::test]
    async fn test_process_stream_token() {
        #[derive(Debug)]
        struct StreamModifier;
        #[async_trait]
        impl AgentPlugin for StreamModifier {
            fn name(&self) -> &str {
                "stream-modifier"
            }
            async fn on_stream_token(
                &self,
                token: StreamTokenWrapper,
            ) -> Option<StreamTokenWrapper> {
                if let StreamTokenWrapper::Text(s) = token {
                    return Some(StreamTokenWrapper::Text(s.replace("hello", "goodbye")));
                }
                None
            }
        }

        let registry = PluginRegistry::new();
        registry.register(Arc::new(StreamModifier)).await;

        let wrapper = StreamTokenWrapper::Text("hello world".to_string());
        let result = registry.process_stream_token(wrapper).await;

        match result {
            StreamTokenWrapper::Text(s) => assert_eq!(s, "goodbye world"),
            _ => panic!("expected modified Text"),
        }
    }

    #[tokio::test]
    async fn test_on_stream_token_tool_call() {
        #[derive(Debug)]
        struct ToolCallModifier;
        #[async_trait]
        impl AgentPlugin for ToolCallModifier {
            fn name(&self) -> &str {
                "tool-call-modifier"
            }
            async fn on_stream_token(
                &self,
                token: StreamTokenWrapper,
            ) -> Option<StreamTokenWrapper> {
                if let StreamTokenWrapper::ToolCall { name, args, id } = token {
                    return Some(StreamTokenWrapper::ToolCall {
                        name: format!("modified_{}", name),
                        args,
                        id,
                    });
                }
                None
            }
        }

        let registry = PluginRegistry::new();
        registry.register(Arc::new(ToolCallModifier)).await;

        let wrapper = StreamTokenWrapper::ToolCall {
            name: "add".to_string(),
            args: serde_json::json!({"a": 1}),
            id: Some("call_123".to_string()),
        };
        let result = registry.process_stream_token(wrapper).await;

        match result {
            StreamTokenWrapper::ToolCall { name, args: _, id } => {
                assert_eq!(name, "modified_add");
                assert_eq!(id, Some("call_123".to_string()));
            }
            _ => panic!("expected modified ToolCall"),
        }
    }

    #[tokio::test]
    async fn test_on_stream_token_none_passes_through() {
        #[derive(Debug)]
        struct NoopStreamPlugin;
        #[async_trait]
        impl AgentPlugin for NoopStreamPlugin {
            fn name(&self) -> &str {
                "noop-stream"
            }
            async fn on_stream_token(
                &self,
                _token: StreamTokenWrapper,
            ) -> Option<StreamTokenWrapper> {
                None
            }
        }

        let registry = PluginRegistry::new();
        registry.register(Arc::new(NoopStreamPlugin)).await;

        let wrapper = StreamTokenWrapper::Text("unchanged".to_string());
        let result = registry.process_stream_token(wrapper).await;

        match result {
            StreamTokenWrapper::Text(s) => assert_eq!(s, "unchanged"),
            _ => panic!("expected unchanged Text"),
        }
    }

    #[tokio::test]
    async fn test_tool_call_wrapper_metadata() {
        let mut wrapper = ToolCallWrapper::new("test_tool", serde_json::json!({}), None);
        wrapper
            .metadata
            .insert("request_id".to_string(), "req_123".to_string());

        assert_eq!(
            wrapper.metadata.get("request_id"),
            Some(&"req_123".to_string())
        );
    }

    #[tokio::test]
    async fn test_llm_request_wrapper_context() {
        use react::llm::LlmMessage;

        let mut context = react::llm::LlmContext::default();
        context
            .conversations
            .push(LlmMessage::user("Hello".to_string()));

        let request = react::llm::LlmRequest {
            model: "gpt-4".to_string(),
            context: context.clone(),
            ..Default::default()
        };

        let wrapper = LlmRequestWrapper::new(&request);
        assert_eq!(wrapper.context.conversations.len(), 1);

        let back = wrapper.into_request();
        assert_eq!(back.context.conversations.len(), 1);
    }

    #[tokio::test]
    async fn test_plugin_panic_isolated() {
        #[derive(Debug)]
        struct PanicPlugin;
        #[async_trait]
        impl AgentPlugin for PanicPlugin {
            fn name(&self) -> &str {
                "panic-plugin"
            }
            async fn on_llm_request(
                &self,
                _request: LlmRequestWrapper,
            ) -> Option<LlmRequestWrapper> {
                panic!("boom");
            }
        }

        #[derive(Debug)]
        struct MarkerPlugin;
        #[async_trait]
        impl AgentPlugin for MarkerPlugin {
            fn name(&self) -> &str {
                "marker"
            }
            async fn on_llm_request(
                &self,
                mut request: LlmRequestWrapper,
            ) -> Option<LlmRequestWrapper> {
                request
                    .metadata
                    .insert("marker".to_string(), "ok".to_string());
                Some(request)
            }
        }

        let registry = PluginRegistry::new();
        registry.register(Arc::new(PanicPlugin)).await;
        registry.register(Arc::new(MarkerPlugin)).await;

        let request = react::llm::LlmRequest {
            model: "gpt-4".to_string(),
            context: Default::default(),
            ..Default::default()
        };
        let wrapper = LlmRequestWrapper::new(&request);
        let result = registry.process_llm_request(wrapper).await;

        assert_eq!(result.metadata.get("marker"), Some(&"ok".to_string()));
    }
}
