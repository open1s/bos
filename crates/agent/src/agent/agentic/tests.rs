    use super::*;
    use crate::agent::context::{AgentReactContext, AgentSession};
    use crate::agent::hooks::{AgentHook, HookContext, HookDecision, HookEvent};
    use crate::tools::FunctionTool;
    use async_trait::async_trait;
    use react::llm::vendor::{ChatCompletionResponse, ChatMessage, Choice};
    use react::llm::{LlmClient, LlmError, LlmRequest, LlmResponse, StreamToken};
    use std::sync::Arc;
    use std::time::Duration;

    /// Mock LLM for testing
    struct MockLlm;

    impl MockLlm {
        fn new() -> Self {
            Self
        }
    }

    fn make_text_response(content: String) -> LlmResponse {
        LlmResponse::OpenAI(ChatCompletionResponse {
            id: "test-mock".to_string(),
            object: "chat.completion".to_string(),
            created: 1234567890,
            model: "mock-model".to_string(),
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: Some(content),
                    tool_calls: None,
                    function_call: None,
                    reasoning_content: None,
                    extra: serde_json::Value::Object(serde_json::Map::new()),
                },
                finish_reason: Some("stop".to_string()),
                stop_reason: None,
                logprobs: None,
            }],
            usage: None,
            system_fingerprint: None,
            nvext: None,
        })
    }

    #[async_trait]
    impl LlmClient<AgentSession, AgentReactContext> for MockLlm {
        async fn complete(
            &self,
            _persona: Option<String>,
            _req: LlmRequest,
            _session: &mut AgentSession,
            _context: &mut AgentReactContext,
        ) -> Result<LlmResponse, LlmError> {
            Ok(make_text_response("mock response".to_string()))
        }

        async fn stream_complete(
            &self,
            _persona: Option<String>,
            _req: LlmRequest,
            _session: &mut AgentSession,
            _context: &mut AgentReactContext,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, LlmError>
        {
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamToken::Text("chunk1".to_string())),
                Ok(StreamToken::Text("chunk2".to_string())),
                Ok(StreamToken::Done),
            ])))
        }

        fn supports_tools(&self) -> bool {
            false
        }

        fn provider_name(&self) -> &'static str {
            "mock"
        }
    }

    fn make_llm_provider() -> LlmProvider {
        let mut provider = LlmProvider::new();
        provider.register_vendor("mock".to_string(), Box::new(MockLlm::new()));
        provider
    }

    // =========================================================================
    // AgentConfig Tests
    // =========================================================================

    #[test]
    fn test_agent_config_default() {
        let config = AgentConfig::default();
        assert_eq!(config.name, "agent");
        assert_eq!(config.model, "gpt-4");
        assert_eq!(config.base_url, "https://api.openai.com/v1");
        assert_eq!(config.api_key, "");
        assert_eq!(config.system_prompt, "You are a helpful assistant.");
        assert_eq!(config.temperature, 0.7);
        assert!(config.max_tokens.is_none());
        assert_eq!(config.timeout_secs, 60);
        assert_eq!(config.max_steps, 10);
        assert!(config.circuit_breaker.is_none());
        assert!(config.rate_limit.is_none());
    }

    // =========================================================================
    // LlmProvider Tests
    // =========================================================================

    #[test]
    fn test_llm_provider_new() {
        let provider = LlmProvider::new();
        drop(provider);
    }

    #[test]
    fn test_llm_provider_register_vendor() {
        let mut provider = LlmProvider::new();
        provider.register_vendor("test".to_string(), Box::new(MockLlm::new()));
    }

    #[test]
    fn test_llm_provider_as_dyn() {
        let provider = Arc::new(LlmProvider::new());
        let _dyn: Box<dyn LlmClient<AgentSession, AgentReactContext>> = provider.as_dyn();
    }

    // =========================================================================
    // Agent Creation Tests
    // =========================================================================

    #[test]
    fn test_agent_new_with_config_and_llm() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let agent = Agent::new(config, Arc::new(provider));

        assert_eq!(agent.config().name, "agent");
        assert_eq!(agent.config().model, "gpt-4");
    }

    #[test]
    fn test_agent_new_with_custom_config() {
        let config = AgentConfig {
            name: "test-agent".to_string(),
            model: "gpt-3.5-turbo".to_string(),
            max_steps: 5,
            ..Default::default()
        };

        let provider = make_llm_provider();
        let agent = Agent::new(config, Arc::new(provider));

        assert_eq!(agent.config().name, "test-agent");
        assert_eq!(agent.config().model, "gpt-3.5-turbo");
        assert_eq!(agent.config().max_steps, 5);
    }

    // =========================================================================
    // Agent Tool Registration Tests
    // =========================================================================

    #[test]
    fn test_agent_add_tool() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let mut agent = Agent::new(config, Arc::new(provider));

        let tool = Arc::new(FunctionTool::new(
            "test_tool",
            "A test tool",
            serde_json::json!({"type":"object"}),
            |_args| Ok(serde_json::json!("result")),
        ));

        agent.add_tool(tool);

        let registry = agent.registry().unwrap();
        assert!(registry.get("test_tool").is_some());
    }

    #[test]
    fn test_agent_add_multiple_tools() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let mut agent = Agent::new(config, Arc::new(provider));

        let tool1 = Arc::new(FunctionTool::new(
            "tool1",
            "First tool",
            serde_json::json!({"type":"object"}),
            |_args| Ok(serde_json::json!("result1")),
        ));

        let tool2 = Arc::new(FunctionTool::new(
            "tool2",
            "Second tool",
            serde_json::json!({"type":"object"}),
            |_args| Ok(serde_json::json!("result2")),
        ));

        agent.add_tool(tool1);
        agent.add_tool(tool2);

        let registry = agent.registry().unwrap();
        assert!(registry.get("tool1").is_some());
        assert!(registry.get("tool2").is_some());
    }

    // =========================================================================
    // Agent Hook Registration Tests
    // =========================================================================

    struct TestHook {
        called: std::sync::Arc<std::sync::Mutex<bool>>,
    }

    impl TestHook {
        fn new() -> Self {
            Self {
                called: Arc::new(std::sync::Mutex::new(false)),
            }
        }
    }

    #[async_trait]
    impl AgentHook for TestHook {
        async fn on_event(&self, _event: HookEvent, _context: &HookContext) -> HookDecision {
            *self.called.lock().unwrap() = true;
            HookDecision::Continue
        }
    }

    #[test]
    fn test_agent_add_hook() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let mut agent = Agent::new(config, Arc::new(provider));

        let hook = Arc::new(TestHook::new());
        agent.add_hook(HookEvent::BeforeToolCall, hook.clone());

        let hooks = agent.hooks();
        let before_hooks = hooks.get_hooks(&HookEvent::BeforeToolCall);
        assert_eq!(before_hooks.len(), 1);
    }

    #[test]
    fn test_agent_add_multiple_hooks() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let mut agent = Agent::new(config, Arc::new(provider));

        let hook1 = Arc::new(TestHook::new());
        let hook2 = Arc::new(TestHook::new());

        agent.add_hook(HookEvent::BeforeToolCall, hook1);
        agent.add_hook(HookEvent::AfterToolCall, hook2);

        let hooks = agent.hooks();
        assert_eq!(hooks.get_hooks(&HookEvent::BeforeToolCall).len(), 1);
        assert_eq!(hooks.get_hooks(&HookEvent::AfterToolCall).len(), 1);
    }

    // =========================================================================
    // Agent Session Tests
    // =========================================================================

    #[test]
    fn test_agent_session_state_empty() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let agent = Agent::new(config, Arc::new(provider));

        let state = agent.session_state();
        assert_eq!(state.agent_id, "agent");
        assert_eq!(state.message_log.len(), 0);
        assert_eq!(state.metadata.message_count, 0);
    }

    #[test]
    fn test_agent_add_message() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let mut agent = Agent::new(config, Arc::new(provider));

        agent.add_message(react::llm::LlmMessage::User {
            content: react::llm::Content::text("Hello"),
        });

        let state = agent.session_state();
        assert_eq!(state.message_log.len(), 1);
        assert_eq!(state.metadata.message_count, 1);
    }

    #[test]
    fn test_agent_add_multiple_messages() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let mut agent = Agent::new(config, Arc::new(provider));

        agent.add_message(react::llm::LlmMessage::User {
            content: react::llm::Content::text("Hello"),
        });
        agent.add_message(react::llm::LlmMessage::assistant("Hi there!"));

        let state = agent.session_state();
        assert_eq!(state.message_log.len(), 2);
    }

    // =========================================================================
    // Agent Clone Tests
    // =========================================================================

    #[test]
    fn test_agent_clone() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let agent = Agent::new(config, Arc::new(provider));

        let cloned = agent.clone();
        assert_eq!(cloned.config().name, agent.config().name);
        assert_eq!(cloned.config().model, agent.config().model);
    }

    // =========================================================================
    // Agent Debug Tests
    // =========================================================================

    #[test]
    fn test_agent_debug() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let agent = Agent::new(config, Arc::new(provider));

        let debug_str = format!("{:?}", agent);
        assert!(debug_str.contains("Agent"));
        assert!(debug_str.contains("agent"));
    }

    // =========================================================================
    // Agent Plugin Tests
    // =========================================================================

    struct TestPlugin;

    impl AgentPlugin for TestPlugin {
        fn name(&self) -> &'static str {
            "test-plugin"
        }
    }

    #[test]
    fn test_agent_add_plugin() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let mut agent = Agent::new(config, Arc::new(provider));

        let plugin = Arc::new(TestPlugin);
        agent.add_plugin(plugin);
    }

    // =========================================================================
    // LlmProvider Default Tests
    // =========================================================================

    #[test]
    fn test_llm_provider_default() {
        let _provider = LlmProvider::default();
    }

    // =========================================================================
    // Metrics Recording Tests
    // =========================================================================

    #[test]
    fn test_metrics_record_call() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let agent = Agent::new(config, Arc::new(provider));

        let before = agent.metrics();
        assert_eq!(before.llm_call_count, 0);
        assert_eq!(before.total_input_tokens, 0);
        assert_eq!(before.total_output_tokens, 0);

        agent.record_stream_call(
            Duration::from_millis(100),
            Duration::from_millis(90),
            Duration::ZERO,
            500,
            300,
        );

        let after = agent.metrics();
        assert_eq!(after.llm_call_count, 1);
        assert_eq!(after.total_input_tokens, 500);
        assert_eq!(after.total_output_tokens, 300);
    }

    #[test]
    fn test_metrics_record_call_accumulates() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let agent = Agent::new(config, Arc::new(provider));

        agent.record_stream_call(
            Duration::from_millis(100),
            Duration::from_millis(90),
            Duration::ZERO,
            500,
            300,
        );
        agent.record_stream_call(
            Duration::from_millis(200),
            Duration::from_millis(180),
            Duration::ZERO,
            1000,
            600,
        );

        let m = agent.metrics();
        assert_eq!(m.llm_call_count, 2);
        assert_eq!(m.total_input_tokens, 1500);
        assert_eq!(m.total_output_tokens, 900);
    }

    #[test]
    fn test_metrics_record_llm_error() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let agent = Agent::new(config, Arc::new(provider));

        let before = agent.metrics();
        assert_eq!(before.llm_errors, 0);

        agent.record_llm_error();
        agent.record_llm_error();

        let after = agent.metrics();
        assert_eq!(after.llm_errors, 2);
    }

    #[test]
    fn test_metrics_record_call_and_error_together() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let agent = Agent::new(config, Arc::new(provider));

        agent.record_stream_call(
            Duration::from_millis(100),
            Duration::from_millis(90),
            Duration::ZERO,
            500,
            300,
        );
        agent.record_llm_error();

        let m = agent.metrics();
        assert_eq!(m.llm_call_count, 1);
        assert_eq!(m.llm_errors, 1);
        assert_eq!(m.total_input_tokens, 500);
        assert_eq!(m.total_output_tokens, 300);
    }

    #[test]
    fn test_last_token_usage_initial() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let agent = Agent::new(config, Arc::new(provider));

        assert!(agent.last_token_usage().is_none());
    }

    #[test]
    fn test_metrics_reset() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let agent = Agent::new(config, Arc::new(provider));

        agent.record_stream_call(
            Duration::from_millis(100),
            Duration::from_millis(90),
            Duration::ZERO,
            500,
            300,
        );
        agent.record_llm_error();

        let before = agent.metrics();
        assert_eq!(before.llm_call_count, 1);
        assert_eq!(before.llm_errors, 1);

        agent.reset_metrics();

        let after = agent.metrics();
        assert_eq!(after.llm_call_count, 0);
        assert_eq!(after.llm_errors, 0);
        assert_eq!(after.total_input_tokens, 0);
        assert_eq!(after.total_output_tokens, 0);
    }

    #[test]
    fn test_metrics_record_tool_calls() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let agent = Agent::new(config, Arc::new(provider));

        let before = agent.metrics();
        assert_eq!(before.tool_invocation_count, 0);

        agent.record_tool_calls(3, Duration::from_millis(150));

        let after = agent.metrics();
        assert_eq!(after.tool_invocation_count, 3);
        assert_eq!(after.total_tool_time.as_millis(), 150);
    }

    #[test]
    fn test_metrics_record_tool_calls_accumulates() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let agent = Agent::new(config, Arc::new(provider));

        agent.record_tool_calls(2, Duration::from_millis(100));
        agent.record_tool_calls(3, Duration::from_millis(200));

        let m = agent.metrics();
        assert_eq!(m.tool_invocation_count, 5);
        assert_eq!(m.total_tool_time.as_millis(), 300);
    }

    #[test]
    fn test_last_stream_tool_calls_initial() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let agent = Agent::new(config, Arc::new(provider));

        assert_eq!(agent.last_stream_tool_calls(), 0);
    }
