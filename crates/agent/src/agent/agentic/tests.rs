use super::*;
use crate::agent::context::{AgentReactContext, AgentSession, CompactionFailure, MAX_STEER_DEPTH};
use crate::agent::hooks::{AgentHook, HookContext, HookDecision, HookEvent};
use crate::tools::FunctionTool;
use async_trait::async_trait;
use react::llm::vendor::{
    ChatCompletionResponse, ChatMessage, Choice, FunctionCall, ToolCall, Usage,
};
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
        usage: Some(Usage {
            prompt_tokens: 10,
            completion_tokens: 20,
            total_tokens: 30,
            prompt_tokens_details: None,
        }),
        system_fingerprint: None,
        nvext: None,
    })
}

/// A response that asks the engine to run one tool.
fn make_tool_call_response(name: &str, arguments: &str) -> LlmResponse {
    LlmResponse::OpenAI(ChatCompletionResponse {
        id: "tool-mock".to_string(),
        object: "chat.completion".to_string(),
        created: 1234567890,
        model: "mock-model".to_string(),
        choices: vec![Choice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: None,
                tool_calls: Some(vec![ToolCall {
                    id: "call-1".to_string(),
                    r#type: "function".to_string(),
                    function: FunctionCall {
                        name: Some(name.to_string()),
                        arguments: Some(arguments.to_string()),
                    },
                }]),
                function_call: None,
                reasoning_content: None,
                extra: serde_json::Value::Object(serde_json::Map::new()),
            },
            finish_reason: Some("tool_calls".to_string()),
            stop_reason: None,
            logprobs: None,
        }],
        usage: Some(Usage {
            prompt_tokens: 10,
            completion_tokens: 20,
            total_tokens: 30,
            prompt_tokens_details: None,
        }),
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
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, LlmError> {
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

/// A vendor that records the persona the engine hands it.
///
/// The persona is the system prompt in this architecture: the engine passes it
/// on every request and the vendor turns it into the request's first system
/// message. Recording it here proves the configured prompt is what the request
/// actually carries, not a value the agent merely holds.
struct PersonaCapturingLlm(Arc<std::sync::Mutex<Option<String>>>);

#[async_trait]
impl LlmClient<AgentSession, AgentReactContext> for PersonaCapturingLlm {
    async fn complete(
        &self,
        persona: Option<String>,
        _req: LlmRequest,
        _session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<LlmResponse, LlmError> {
        *self.0.lock().unwrap() = persona;
        Ok(make_text_response("ok".to_string()))
    }

    async fn stream_complete(
        &self,
        persona: Option<String>,
        _req: LlmRequest,
        _session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, LlmError> {
        *self.0.lock().unwrap() = persona;
        Ok(Box::pin(futures::stream::iter(vec![Ok(StreamToken::Done)])))
    }

    fn provider_name(&self) -> &'static str {
        "persona-capturing-mock"
    }
}

/// The system prompt is not merely stored on the config: it is the persona the
/// request carries, identity first.
#[tokio::test]
async fn the_system_prompt_is_the_persona_the_request_carries() {
    let seen = Arc::new(std::sync::Mutex::new(None));
    let mut provider = LlmProvider::new();
    provider.register_vendor(
        "mock".to_string(),
        Box::new(PersonaCapturingLlm(seen.clone())),
    );
    let agent = Agent::new(AgentConfig::default(), Arc::new(provider));
    let _ = agent.react("task".to_string()).await;
    let persona = seen
        .lock()
        .unwrap()
        .clone()
        .expect("the request carried a persona");
    assert_eq!(persona, AgentConfig::default().system_prompt);
    assert!(persona.starts_with(crate::agent::prompts::HARNESS_IDENTITY));
    assert!(persona.contains(crate::agent::prompts::WORKFLOW_GUIDANCE));
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
    assert_eq!(
        config.system_prompt,
        super::super::agentic::DEFAULT_SYSTEM_PROMPT
    );
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

// =========================================================================
// Observer -> MetricsCollector wiring tests (the production LLM path)
// =========================================================================

#[tokio::test]
async fn react_records_real_rate_limit_waits() {
    let config = AgentConfig::default().rate_limit(RateLimiterConfig {
        capacity: 1,
        window: Duration::from_millis(150),
        max_retries: 3,
        retry_backoff: Duration::from_millis(20),
        auto_wait: true,
    });
    let provider = make_llm_provider();
    let agent = Agent::new(config, Arc::new(provider));

    // The first run consumes the single rate-limit slot; the second run
    // blocks in `ReActResilience::acquire` until the window slides, and that
    // sleep must reach the collector.
    let _ = agent.react("first run".to_string()).await;
    let _ = agent.react("second run".to_string()).await;

    let m = agent.metrics();
    assert!(
        m.rate_limit_waits >= 1,
        "the second run must block in the limiter; rate_limit_waits = {}",
        m.rate_limit_waits
    );
    assert!(
        m.total_rate_limit_wait > Duration::ZERO,
        "the observed wait must carry its duration"
    );
    assert!(
        m.total_resilience_time > Duration::ZERO,
        "rate limit waits must flow into total_resilience_time"
    );
}

/// Mock LLM that reports a rate limit on its first call so the engine's
/// retry loop actually runs, then succeeds.
struct FlakyLlm {
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl LlmClient<AgentSession, AgentReactContext> for FlakyLlm {
    async fn complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        _session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<LlmResponse, LlmError> {
        if self
            .calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            == 0
        {
            return Err(LlmError::RateLimited);
        }
        Ok(make_text_response("recovered".to_string()))
    }

    async fn stream_complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        _session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, LlmError> {
        Ok(Box::pin(futures::stream::iter(vec![Ok(StreamToken::Done)])))
    }

    fn supports_tools(&self) -> bool {
        false
    }

    fn provider_name(&self) -> &'static str {
        "flaky"
    }
}

#[tokio::test]
async fn retry_backoff_reaches_the_metrics() {
    let config = AgentConfig::default().rate_limit(RateLimiterConfig {
        capacity: 10,
        window: Duration::from_secs(1),
        max_retries: 3,
        retry_backoff: Duration::from_millis(25),
        auto_wait: true,
    });
    let mut provider = LlmProvider::new();
    provider.register_vendor(
        "flaky".to_string(),
        Box::new(FlakyLlm {
            calls: std::sync::atomic::AtomicUsize::new(0),
        }),
    );
    let agent = Agent::new(config, Arc::new(provider));

    // The first LLM call is rate limited, so `call_llm` retries through
    // `ReActResilience::backoff`, whose sleep must be reported.
    let _ = agent.react("run".to_string()).await;

    let m = agent.metrics();
    assert!(
        m.total_resilience_time >= Duration::from_millis(25),
        "the retry backoff sleep must be reported; total_resilience_time = {:?}",
        m.total_resilience_time
    );
}

// =========================================================================
// Per-run metric deltas on a cached engine
// =========================================================================

#[tokio::test]
async fn react_records_per_run_token_deltas() {
    let provider = make_llm_provider();
    let agent = Agent::new(AgentConfig::default(), Arc::new(provider));

    let _ = agent.react("first".to_string()).await;
    let m1 = agent.metrics();
    assert_eq!(m1.total_input_tokens, 10);
    assert_eq!(m1.total_output_tokens, 20);
    assert_eq!(m1.llm_call_count, 1);

    // The second run reuses the cached engine whose counters accumulated;
    // it must record its own tokens, not the cumulative total.
    let _ = agent.react("second".to_string()).await;
    let m2 = agent.metrics();
    assert_eq!(m2.total_input_tokens, 20);
    assert_eq!(m2.total_output_tokens, 40);
    assert_eq!(m2.llm_call_count, 2);
}

/// Mock LLM that requests a tool on the first call of every run, then
/// answers with plain text.
struct ToolCallLlm {
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl LlmClient<AgentSession, AgentReactContext> for ToolCallLlm {
    async fn complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        _session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<LlmResponse, LlmError> {
        let call = self
            .calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if call.is_multiple_of(2) {
            Ok(make_tool_call_response("slow_tool", r#"{"input":"hi"}"#))
        } else {
            Ok(make_text_response("final".to_string()))
        }
    }

    async fn stream_complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        _session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, LlmError> {
        Ok(Box::pin(futures::stream::iter(vec![Ok(StreamToken::Done)])))
    }

    fn supports_tools(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "toolmock"
    }
}

#[tokio::test]
async fn react_records_per_run_tool_deltas() {
    let mut provider = LlmProvider::new();
    provider.register_vendor(
        "toolmock".to_string(),
        Box::new(ToolCallLlm {
            calls: std::sync::atomic::AtomicUsize::new(0),
        }),
    );
    let mut agent = Agent::new(AgentConfig::default(), Arc::new(provider));
    agent.add_tool(Arc::new(FunctionTool::new(
        "slow_tool",
        "A tool that takes a measurable amount of time",
        serde_json::json!({"type":"object"}),
        |_args| {
            std::thread::sleep(Duration::from_millis(2));
            Ok(serde_json::json!("done"))
        },
    )));

    let _ = agent.react("first".to_string()).await;
    assert_eq!(agent.metrics().tool_invocation_count, 1);

    // The engine's counter accumulated to 2; the second run must record the
    // one tool call it added (1 + 1), not the cumulative total (1 + 2).
    let _ = agent.react("second".to_string()).await;
    let m = agent.metrics();
    assert_eq!(m.tool_invocation_count, 2);
    assert!(
        m.total_tool_time >= Duration::from_millis(2),
        "tool execution time must be recorded; total_tool_time = {:?}",
        m.total_tool_time
    );
}

/* ---- MCP attach (Agent::add_mcp_tool) ---- */

/// A lazily-created HTTP client: no network I/O happens until a request is
/// made, which these registration tests never do.
fn lazy_mcp_client() -> Arc<crate::mcp::McpClient> {
    Arc::new(crate::mcp::McpClient::connect_http("http://127.0.0.1:9"))
}

/// Build an adapter whose registry name is `{namespace}_{tool}`.
fn mcp_adapter(
    client: &Arc<crate::mcp::McpClient>,
    namespace: &str,
    tool: &str,
) -> Arc<dyn react::tool::registry::AsyncTool> {
    Arc::new(crate::mcp::McpToolAdapter::new(
        client.clone(),
        format!("{namespace}_{tool}"),
        tool.to_string(),
        "test tool".to_string(),
        serde_json::json!({"type": "object"}),
    ))
}

#[test]
fn add_mcp_tool_rejects_invalid_names() {
    let mut agent = Agent::from_config(AgentConfig::default());
    let client = lazy_mcp_client();

    let empty_ns = agent.add_mcp_tool("  ", "t", mcp_adapter(&client, "x", "t"));
    assert!(empty_ns.is_err(), "blank namespace must be rejected");

    let slash_ns = agent.add_mcp_tool("bad/ns", "t", mcp_adapter(&client, "x", "t"));
    assert!(slash_ns.is_err(), "'/' must be rejected");

    let space_ns = agent.add_mcp_tool("bad ns", "t", mcp_adapter(&client, "x", "t"));
    assert!(space_ns.is_err(), "spaces must be rejected");

    let empty_tool = agent.add_mcp_tool("ok", " ", mcp_adapter(&client, "ok", "t"));
    assert!(empty_tool.is_err(), "blank tool name must be rejected");

    // The adapter's own name must match `{namespace}_{tool_name}`.
    let mismatch = agent.add_mcp_tool("ns", "other", mcp_adapter(&client, "ns", "t"));
    assert!(mismatch.is_err(), "name mismatch must be rejected");

    assert!(
        agent
            .registry()
            .is_none_or(|reg| reg.async_tool_names().is_empty()),
        "no failed attempt may register a tool"
    );
}

#[test]
fn add_mcp_tool_registers_marks_and_lists() {
    let mut agent = Agent::from_config(AgentConfig::default());
    let client = lazy_mcp_client();

    agent
        .add_mcp_tool("files", "read", mcp_adapter(&client, "files", "read"))
        .expect("registration must succeed");

    let reg = agent.registry().expect("registry exists after attach");
    let full = "files_read";
    assert!(reg.async_tool_names().contains(&full.to_string()));
    assert!(reg.is_mcp_tool(full), "{full} must be marked MCP");
    assert!(
        reg.mcp_tool_entries()
            .iter()
            .any(|e| e.get("name").and_then(|n| n.as_str()) == Some(full)),
        "mcp_tool_entries must project {full}"
    );

    let dup = agent.add_mcp_tool("files", "read", mcp_adapter(&client, "files", "read"));
    assert!(dup.is_err(), "re-attaching the same tool must fail");
}

// =========================================================================
// Auto-remember Tests
// =========================================================================

#[test]
fn auto_remember_is_off_by_default() {
    let agent = Agent::new(AgentConfig::default(), Arc::new(make_llm_provider()));
    assert!(!agent.auto_remember(), "off unless explicitly enabled");
}

/// A run finished through `react` with auto-remember on lands in the
/// attached file store and is recalled by a fresh agent on the same file.
#[tokio::test]
async fn auto_remember_persists_completed_exchange() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("bos-auto-remember-{}-{nanos}", std::process::id()));
    let path = dir.join("memory.jsonl");

    {
        let mut agent = Agent::new(AgentConfig::default(), Arc::new(make_llm_provider()));
        let memory = crate::memory::FileMemory::open(&path).await.expect("open");
        agent = agent.with_memory(Arc::new(memory));
        agent.set_auto_remember(true);
        assert!(agent.auto_remember());
        let _ = agent
            .react("remember the codename is zephyr".to_string())
            .await
            .expect("react succeeds");
    }

    // A fresh agent (new process equivalent) reads the same file back.
    let mut agent = Agent::new(AgentConfig::default(), Arc::new(make_llm_provider()));
    let memory = crate::memory::FileMemory::open(&path)
        .await
        .expect("reopen");
    agent = agent.with_memory(Arc::new(memory));
    let recalled = agent
        .recalled_context("codename zephyr")
        .await
        .expect("the stored exchange must recall");
    assert!(recalled.contains("zephyr"), "prompt half: {recalled}");
    assert!(recalled.contains("mock response"), "reply half: {recalled}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// With auto-remember off (the default), a completed run writes nothing.
#[tokio::test]
async fn auto_remember_off_leaves_memory_empty() {
    let mut agent = Agent::new(AgentConfig::default(), Arc::new(make_llm_provider()));
    let memory = crate::memory::InMemoryMemory::new();
    agent = agent.with_memory(Arc::new(memory));
    let _ = agent
        .react("task".to_string())
        .await
        .expect("react succeeds");

    let items = agent
        .memory()
        .expect("memory attached")
        .search("task", 5)
        .await;
    assert!(items.is_empty(), "nothing stored unless enabled: {items:?}");
}

/// The streaming entry point records the exchange too, so chat UIs (which
/// only ever call `stream`) get persistence without doing it themselves.
#[tokio::test]
async fn auto_remember_records_streamed_exchange() {
    let mut agent = Agent::new(AgentConfig::default(), Arc::new(make_llm_provider()));
    let memory = crate::memory::InMemoryMemory::new();
    agent = agent.with_memory(Arc::new(memory));
    agent.set_auto_remember(true);

    let mut stream = agent.stream("stream this".to_string());
    use futures::StreamExt;
    let mut text = String::new();
    while let Some(item) = stream.next().await {
        if let Ok(StreamToken::Text(chunk)) = item {
            text.push_str(&chunk);
        }
    }
    assert!(!text.is_empty(), "the mock stream must yield text");

    let items = agent
        .memory()
        .expect("memory attached")
        .search("stream", 5)
        .await;
    assert_eq!(items.len(), 1, "exactly one exchange note: {items:?}");
    assert!(
        items[0].content.contains("stream this"),
        "{}",
        items[0].content
    );
    assert!(items[0].content.contains(&text), "reply captured verbatim");
}

/// A stream that is never polled must not take the transcript with it.
#[test]
fn dropping_an_unpolled_stream_keeps_the_session_history() {
    let mut agent = Agent::new(AgentConfig::default(), Arc::new(make_llm_provider()));
    agent
        .session_mut()
        .push(react::llm::LlmMessage::user_text("earlier"));

    let stream = agent.stream("next");
    drop(stream);

    let state = agent.session_state();
    assert_eq!(state.message_log.len(), 1, "the earlier turn survives");
}

/// Chat UIs commonly stop at `Done` and drop the stream; the session and the
/// exchange must still be restored rather than lost with the generator.
#[tokio::test]
async fn stopping_at_done_keeps_the_session_history() {
    use futures::StreamExt;

    let mut agent = Agent::new(AgentConfig::default(), Arc::new(make_llm_provider()));
    let memory = crate::memory::InMemoryMemory::new();
    agent = agent.with_memory(Arc::new(memory));
    agent.set_auto_remember(true);
    agent
        .session_mut()
        .push(react::llm::LlmMessage::user_text("earlier"));

    {
        let mut stream = agent.stream("next");
        while let Some(item) = stream.next().await {
            if matches!(item, Ok(StreamToken::Done)) {
                break;
            }
        }
    }

    let state = agent.session_state();
    assert!(
        state.message_log.len() > 1,
        "the run's messages are restored, not discarded: {}",
        state.message_log.len()
    );
    let items = agent
        .memory()
        .expect("memory attached")
        .search("next", 5)
        .await;
    assert_eq!(
        items.len(),
        1,
        "the exchange is recorded before Done is handed out: {items:?}"
    );
}

// ============================================================================
// Working-plan (update_plan) Tests

/// The plan store is shared: a `PlanTool` built from `agent.plan()` is
/// immediately visible through `agent.plan_items()`.
#[test]
fn plan_tool_shares_the_agents_store() {
    let agent = Agent::new(AgentConfig::default(), Arc::new(make_llm_provider()));
    let tool = crate::tools::PlanTool::new(agent.plan());
    let out = tool
        .run(&serde_json::json!({
            "items": [
                {"text": "explore the repo", "status": "completed"},
                {"text": "write the fix", "status": "in_progress"}
            ]
        }))
        .expect("plan update");
    assert_eq!(out["total"], 2);

    let items = agent.plan_items();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].text, "explore the repo");
    assert_eq!(items[0].status, crate::tools::PlanStatus::Completed);
    assert_eq!(items[1].status, crate::tools::PlanStatus::InProgress);
}

/// Hosts restore a persisted plan with `set_plan`; an empty list clears it.
#[test]
fn set_plan_restores_and_clears() {
    use crate::tools::{PlanItem, PlanStatus};
    let agent = Agent::new(AgentConfig::default(), Arc::new(make_llm_provider()));
    agent.set_plan(vec![PlanItem {
        text: "restored".to_string(),
        status: PlanStatus::Completed,
    }]);
    assert_eq!(agent.plan_items().len(), 1);

    agent.set_plan(Vec::new());
    assert!(agent.plan_items().is_empty());
}

/// The stateless clone keeps the same plan store, so a rebuilt agent in a
/// restarted host still shares in-flight planning state.
#[test]
fn cloned_agent_shares_the_plan() {
    use crate::tools::{PlanItem, PlanStatus};
    let agent = Agent::new(AgentConfig::default(), Arc::new(make_llm_provider()));
    let clone = agent.clone();
    clone.set_plan(vec![PlanItem {
        text: "shared".to_string(),
        status: PlanStatus::Pending,
    }]);
    assert_eq!(agent.plan_items().len(), 1);
}

// =========================================================================
// Multi-provider failover tests (the ordered fallback chain)
// =========================================================================

/// Mock LLM that always fails, standing in for a dead endpoint.
struct DeadLlm;

#[async_trait]
impl LlmClient<AgentSession, AgentReactContext> for DeadLlm {
    async fn complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        _session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<LlmResponse, LlmError> {
        Err(LlmError::RateLimited)
    }

    async fn stream_complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        _session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, LlmError> {
        Err(LlmError::Timeout)
    }

    fn supports_tools(&self) -> bool {
        false
    }

    fn provider_name(&self) -> &'static str {
        "dead"
    }
}

/// Mock LLM that records the model id it received, so a test can verify
/// both that a fallback ran and that the vendor prefix was stripped.
struct ModelEchoLlm {
    seen: Arc<std::sync::Mutex<String>>,
}

#[async_trait]
impl LlmClient<AgentSession, AgentReactContext> for ModelEchoLlm {
    async fn complete(
        &self,
        _persona: Option<String>,
        req: LlmRequest,
        _session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<LlmResponse, LlmError> {
        *self.seen.lock().unwrap() = req.model.clone();
        Ok(make_text_response("fallback ok".to_string()))
    }

    async fn stream_complete(
        &self,
        _persona: Option<String>,
        req: LlmRequest,
        _session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, LlmError> {
        *self.seen.lock().unwrap() = req.model.clone();
        Ok(Box::pin(futures::stream::iter(vec![Ok(StreamToken::Done)])))
    }

    fn supports_tools(&self) -> bool {
        false
    }

    fn provider_name(&self) -> &'static str {
        "echo"
    }
}

/// A dead primary must transparently fail over to the registered fallback,
/// which receives the vendor-stripped model id just like the primary would.
#[tokio::test]
async fn failover_sends_to_fallback_when_primary_fails() {
    let mut provider = LlmProvider::new();
    provider.register_vendor("dead".to_string(), Box::new(DeadLlm));
    let seen = Arc::new(std::sync::Mutex::new(String::new()));
    provider.register_fallback(
        "vendor-b/local-model".to_string(),
        Box::new(ModelEchoLlm { seen: seen.clone() }),
    );

    let mut session = AgentSession::default();
    let mut context = AgentReactContext::new("failover-1".to_string());
    let response = provider
        .complete(
            None,
            LlmRequest::new("dead/primary-model"),
            &mut session,
            &mut context,
        )
        .await;

    assert!(
        response.is_ok(),
        "the fallback must answer when the primary fails: {response:?}"
    );
    assert_eq!(
        *seen.lock().unwrap(),
        "local-model",
        "the fallback must receive the vendor-stripped model id"
    );
}

/// When every provider in the chain fails, one aggregated error names each
/// attempt so the operator can see which endpoints broke.
#[tokio::test]
async fn failover_reports_every_provider_when_all_fail() {
    let mut provider = LlmProvider::new();
    provider.register_vendor("dead".to_string(), Box::new(DeadLlm));
    provider.register_fallback("also-dead/local-model".to_string(), Box::new(DeadLlm));

    let mut session = AgentSession::default();
    let mut context = AgentReactContext::new("failover-2".to_string());
    let err = provider
        .complete(
            None,
            LlmRequest::new("dead/primary-model"),
            &mut session,
            &mut context,
        )
        .await
        .expect_err("an all-dead chain must surface an error");

    let msg = err.to_string();
    assert!(
        msg.contains("all 2 provider(s) failed"),
        "the aggregate must count every attempt: {msg}"
    );
}

/// A healthy primary must never touch the fallbacks.
#[tokio::test]
async fn failover_skips_fallback_when_primary_succeeds() {
    let mut provider = LlmProvider::new();
    provider.register_vendor("mock".to_string(), Box::new(MockLlm::new()));
    let seen = Arc::new(std::sync::Mutex::new(String::new()));
    provider.register_fallback(
        "other/should-not-run".to_string(),
        Box::new(ModelEchoLlm { seen: seen.clone() }),
    );

    let mut session = AgentSession::default();
    let mut context = AgentReactContext::new("failover-3".to_string());
    let response = provider
        .complete(
            None,
            LlmRequest::new("mock/primary-model"),
            &mut session,
            &mut context,
        )
        .await;

    assert!(
        response.is_ok(),
        "the primary answers on its own: {response:?}"
    );
    assert!(
        seen.lock().unwrap().is_empty(),
        "the fallback must not run when the primary succeeds"
    );
}

/// The config surface: fallbacks default to an empty chain and the fluent
/// setter appends entries in order.
#[test]
fn test_agent_config_fallbacks_default_empty_and_setter() {
    let config = AgentConfig::default();
    assert!(config.fallbacks.is_empty());

    let config = config.fallback(FallbackProvider {
        base_url: "http://127.0.0.1:1/v1".to_string(),
        api_key: String::new(),
        model: "local/backup".to_string(),
    });
    assert_eq!(config.fallbacks.len(), 1);
    assert_eq!(config.fallbacks[0].model, "local/backup");
    assert_eq!(config.fallbacks[0].base_url, "http://127.0.0.1:1/v1");
}

#[test]
fn build_vendor_routes_the_anthropic_prefix_to_the_messages_vendor() {
    let config = AgentConfig {
        name: "anthropic-agent".to_string(),
        model: "anthropic/claude-3-5-sonnet-latest".to_string(),
        ..Default::default()
    };
    let (name, vendor) = super::llm::build_vendor(&config);
    assert_eq!(name, "anthropic");
    assert_eq!(vendor.provider_name(), "anthropic");
    assert!(vendor.supports_tools());
}

#[test]
fn build_vendor_keeps_openai_as_the_unprefixed_default() {
    let config = AgentConfig {
        name: "plain-agent".to_string(),
        model: "gpt-4".to_string(),
        ..Default::default()
    };
    let (name, vendor) = super::llm::build_vendor(&config);
    assert_eq!(name, "openai");
    assert_eq!(vendor.provider_name(), "openai");
}

/// The summarizer seam both the manual command and the automatic pressure
/// trigger drive: one model call whose returned TEXT alone becomes the
/// checkpoint, replacing the older span and keeping the newest message.
#[tokio::test]
async fn compact_now_installs_a_model_written_checkpoint() {
    let agent = Agent::new(AgentConfig::default(), Arc::new(make_llm_provider()));
    {
        let mut session = agent.session();
        // Long enough that the framed checkpoint (the preamble alone is ~75
        // tokens) is genuinely smaller than the span it replaces.
        session.add_user(format!("old one {}", "x".repeat(400)));
        session.add_assistant(format!("reply one {}", "y".repeat(400)));
        session.add_user(format!("old two {}", "x".repeat(400)));
        session.add_assistant(format!("reply two {}", "y".repeat(400)));
    }
    let signal = std::sync::atomic::AtomicBool::new(false);
    let outcome = agent
        .compact_now(0, &signal)
        .await
        .expect("the compaction request")
        .expect("a checkpoint");
    assert_eq!(outcome.replaced, 3, "all but the last message");
    assert_eq!(outcome.summary, "mock response");
    let session = agent.session();
    assert_eq!(
        session.messages().len(),
        2,
        "the framed checkpoint plus the kept reply"
    );
    match session.messages().first() {
        Some(react::LlmMessage::User { content }) => match content {
            react::llm::Content::Text(text) => {
                assert!(text.contains("<compacted-summary>"));
                assert!(text.contains("mock response"));
                assert!(text.ends_with("</compacted-summary>"));
            }
            _ => panic!("the checkpoint is text"),
        },
        other => panic!("the checkpoint is a user message: {other:?}"),
    }
    assert!(
        matches!(
            session.messages().last(),
            Some(react::LlmMessage::Assistant { content }) if content.starts_with("reply two")
        ),
        "the newest message is kept verbatim"
    );
}

/// The overflow request carries the pinned compaction instruction, not a copy
/// that drifted: the summarizer's final user message is the digest, a
/// separator, then `COMPACTION_INSTRUCTION` byte for byte.
#[tokio::test]
async fn the_compaction_request_carries_the_pinned_instruction() {
    let (agent, seen) = agent_with_script(Ok("checkpoint".to_string()), 0);
    {
        let mut session = agent.session();
        session.add_user(format!("old one {}", "x".repeat(400)));
        session.add_assistant(format!("reply one {}", "y".repeat(400)));
        session.add_user(format!("old two {}", "x".repeat(400)));
        session.add_assistant(format!("reply two {}", "y".repeat(400)));
    }
    let signal = std::sync::atomic::AtomicBool::new(false);
    let _ = agent
        .compact_now(0, &signal)
        .await
        .expect("the compaction request")
        .expect("a checkpoint");
    let prompts = seen.lock().unwrap();
    assert_eq!(prompts.len(), 1, "one summarizer request");
    let prompt = &prompts[0];
    assert!(
        prompt.ends_with(crate::agent::context::COMPACTION_INSTRUCTION),
        "the pinned instruction is the request's tail"
    );
    assert!(
        prompt.contains("\n\n---\n\n"),
        "the digest and the instruction are separated"
    );
    assert!(
        !prompt.starts_with(crate::agent::context::COMPACTION_INSTRUCTION),
        "the conversation digest comes before the instruction"
    );
}

/// A vendor that always answers the summarizer with one fixed text.
struct FixedSummaryMock(String);

#[async_trait]
impl LlmClient<AgentSession, AgentReactContext> for FixedSummaryMock {
    async fn complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        _session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<LlmResponse, LlmError> {
        Ok(make_text_response(self.0.clone()))
    }

    async fn stream_complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        _session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, LlmError> {
        Ok(Box::pin(futures::stream::iter(vec![Ok(StreamToken::Done)])))
    }

    fn provider_name(&self) -> &'static str {
        "fixed-summary-mock"
    }
}

fn agent_with_summary(summary: &str) -> Agent {
    let mut provider = LlmProvider::new();
    provider.register_vendor(
        "mock".to_string(),
        Box::new(FixedSummaryMock(summary.to_string())),
    );
    Agent::new(AgentConfig::default(), Arc::new(provider))
}

/// A summarizer stand-in that records the request it was handed and answers
/// with either fixed text or a provider error.
struct ScriptedSummaryMock {
    answer: Result<String, LlmError>,
    seen: Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait]
impl LlmClient<AgentSession, AgentReactContext> for ScriptedSummaryMock {
    async fn complete(
        &self,
        _persona: Option<String>,
        req: LlmRequest,
        _session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<LlmResponse, LlmError> {
        let text = match req.input {
            react::llm::Content::Text(text) => text,
            react::llm::Content::Parts(parts) => parts
                .into_iter()
                .filter_map(|part| match part {
                    react::llm::ContentPart::Text { text } => Some(text),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" "),
        };
        self.seen.lock().unwrap().push(text);
        match &self.answer {
            Ok(summary) => Ok(make_text_response(summary.clone())),
            Err(error) => Err(error.clone()),
        }
    }

    async fn stream_complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        _session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, LlmError> {
        Ok(Box::pin(futures::stream::iter(vec![Ok(StreamToken::Done)])))
    }

    fn provider_name(&self) -> &'static str {
        "scripted-summary-mock"
    }
}

/// Build an agent whose summarizer is scripted, with an optional input budget.
fn agent_with_script(
    answer: Result<String, LlmError>,
    compaction_input_tokens: usize,
) -> (Agent, Arc<std::sync::Mutex<Vec<String>>>) {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut provider = LlmProvider::new();
    provider.register_vendor(
        "mock".to_string(),
        Box::new(ScriptedSummaryMock {
            answer,
            seen: seen.clone(),
        }),
    );
    let agent = Agent::new(
        AgentConfig {
            compaction_input_tokens,
            ..AgentConfig::default()
        },
        Arc::new(provider),
    );
    (agent, seen)
}

fn four_messages(agent: &Agent) {
    let mut session = agent.session();
    session.add_user("old one".to_string());
    session.add_assistant("reply one".to_string());
    session.add_user("old two".to_string());
    session.add_assistant("reply two".to_string());
}

#[tokio::test]
async fn compact_now_refuses_an_empty_summary() {
    let agent = agent_with_summary("");
    four_messages(&agent);
    let signal = std::sync::atomic::AtomicBool::new(false);
    let error = agent
        .compact_now(0, &signal)
        .await
        .expect_err("an empty answer is a summary failure");
    assert!(matches!(
        error,
        AgentError::Compaction(CompactionFailure::EmptySummary)
    ));
    assert!(error.to_string().contains("no text summary content"));
    assert_eq!(
        agent.session().messages().len(),
        4,
        "a failed summary leaves the history untouched"
    );
}

// =========================================================================
// Steering: a host message taken at the RUNNING turn's next step
// =========================================================================

/// A provider error keeps its own text and is classified as a provider
/// failure, never as a "useless summary": the reader has to see whether the
/// request was too large, malformed or unauthorised.
#[tokio::test]
async fn compact_now_classifies_a_provider_failure_and_keeps_its_text() {
    let (agent, _) = agent_with_script(
        Err(LlmError::Other(
            "HTTP 400: maximum context length exceeded (32768 tokens)".to_string(),
        )),
        24_000,
    );
    four_messages(&agent);
    let signal = std::sync::atomic::AtomicBool::new(false);
    let error = agent
        .compact_now(0, &signal)
        .await
        .expect_err("the provider rejected the request");
    match &error {
        AgentError::Compaction(CompactionFailure::Provider {
            detail,
            over_window,
        }) => {
            assert!(*over_window, "the text names a context length: {detail}");
            assert!(detail.contains("maximum context length exceeded"));
        }
        other => panic!("a provider failure, not {other:?}"),
    }
    assert!(error
        .to_string()
        .contains("maximum context length exceeded"));
    assert_eq!(
        agent.session().messages().len(),
        4,
        "a provider failure leaves the history untouched"
    );
}

/// A missing credential is its own class: the row must not tell the reader the
/// summary was useless when the request was never authorised.
#[tokio::test]
async fn compact_now_classifies_a_missing_credential() {
    let (agent, _) = agent_with_script(Err(LlmError::ApiKeyMissing), 24_000);
    four_messages(&agent);
    let signal = std::sync::atomic::AtomicBool::new(false);
    let error = agent
        .compact_now(0, &signal)
        .await
        .expect_err("no credential");
    assert!(matches!(
        error,
        AgentError::Compaction(CompactionFailure::MissingCredential { .. })
    ));
}

/// A span larger than the configured budget is sent as its OLDEST slice plus
/// its NEWEST slice: the request is bounded, both ends survive, and the
/// compaction still lands.
#[tokio::test]
async fn compact_now_bounds_a_span_larger_than_the_request_budget() {
    const BUDGET: usize = 1_500;
    let (agent, seen) = agent_with_script(Ok("brief checkpoint".to_string()), BUDGET);
    {
        let mut session = agent.session();
        for index in 0..8 {
            let body = format!("old {index} {} tail{index}", "x".repeat(2_000));
            if index % 2 == 0 {
                session.add_user(body);
            } else {
                session.add_assistant(body);
            }
        }
    }
    let signal = std::sync::atomic::AtomicBool::new(false);
    let outcome = agent
        .compact_now(0, &signal)
        .await
        .expect("the bounded compaction")
        .expect("a checkpoint");
    assert_eq!(outcome.replaced, 7, "all but the retained last message");
    let prompt = seen.lock().unwrap().last().cloned().expect("one request");
    let priced = crate::agent::context::estimate_tokens(&prompt);
    assert!(
        priced <= BUDGET,
        "the whole request fits the configured budget: {priced} > {BUDGET}"
    );
    assert!(
        prompt.contains("old 0"),
        "the oldest intent survives the bound"
    );
    assert!(
        prompt.contains("tail6"),
        "the newest work survives the bound"
    );
    assert!(
        !prompt.contains("tail3"),
        "the middle is what the budget omits: {}",
        prompt.len()
    );
    assert!(
        prompt.contains(crate::agent::context::COMPACTION_OMISSION),
        "the omission is stated, not silent"
    );
}

/// The user turns a session carries, in order, as the mock sees it.
fn user_turns(session: &AgentSession) -> Vec<String> {
    session
        .messages()
        .iter()
        .filter_map(|message| match message {
            react::LlmMessage::User {
                content: Content::Text(text),
            } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// A mock that records the session at every call and asks for one tool on the
/// first, so the engine reaches a SECOND step - the only place a steer can land.
struct SteerReceivingLlm {
    calls: std::sync::atomic::AtomicUsize,
    seen: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
}

#[async_trait]
impl LlmClient<AgentSession, AgentReactContext> for SteerReceivingLlm {
    async fn complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        _session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<LlmResponse, LlmError> {
        Ok(make_text_response("blocking path unused".to_string()))
    }

    async fn stream_complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, LlmError> {
        self.seen.lock().unwrap().push(user_turns(session));
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if call == 0 {
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamToken::ToolCall {
                    name: "no_such_tool".to_string(),
                    args: serde_json::json!({}),
                    id: Some("steer-call-1".to_string()),
                }),
                Ok(StreamToken::Done),
            ])))
        } else {
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamToken::Text("acknowledged".to_string())),
                Ok(StreamToken::Done),
            ])))
        }
    }

    fn supports_tools(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "steer-mock"
    }
}

/// A hook that steers once, on the step the engine is about to run.
struct SteerOnFirstStep {
    mailbox: Arc<SteerMailbox>,
    fired: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl AgentHook for SteerOnFirstStep {
    async fn on_event(&self, _event: HookEvent, _context: &HookContext) -> HookDecision {
        if !self.fired.swap(true, std::sync::atomic::Ordering::SeqCst) {
            assert!(
                self.mailbox.push(SteerMessage {
                    content: Content::Text("steer now".to_string()),
                    ticket: 7,
                }),
                "the mailbox takes the steer"
            );
        }
        HookDecision::Continue
    }
}

/// A steer pushed while step one is in flight is appended as a USER TURN
/// before step two's request, and the sink reports the ticket at that point.
///
/// The assertion is on the session the mock is handed - which is what the
/// vendor turns into the wire request - not on a flag: call one must not carry
/// the message, call two must.
#[tokio::test]
async fn a_steered_message_enters_the_next_request_as_a_user_turn() {
    let seen = Arc::new(std::sync::Mutex::new(Vec::<Vec<String>>::new()));
    let tickets = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
    let mut provider = LlmProvider::new();
    provider.register_vendor(
        "mock".to_string(),
        Box::new(SteerReceivingLlm {
            calls: std::sync::atomic::AtomicUsize::new(0),
            seen: Arc::clone(&seen),
        }),
    );
    let agent = Agent::new(AgentConfig::default(), Arc::new(provider));
    // The mailbox the APP drains, not a fresh one: a steer the engine never
    // reads would prove nothing.
    let mailbox = agent.steers().clone();
    agent.hooks().register(
        HookEvent::BeforeLlmCall,
        Arc::new(SteerOnFirstStep {
            mailbox: Arc::clone(&mailbox),
            fired: std::sync::atomic::AtomicBool::new(false),
        }),
    );
    let sink_tickets = Arc::clone(&tickets);
    agent.on_steered(move |ticket| sink_tickets.lock().unwrap().push(ticket));
    // The agent's own mailbox must be the one the app drains, or the hook's
    // push would go somewhere the engine never looks.
    assert_eq!(mailbox.depth(), 0);

    let stream = agent.stream("start the turn");
    futures::pin_mut!(stream);
    while let Some(item) = stream.next().await {
        assert!(item.is_ok(), "the turn streams cleanly: {item:?}");
    }

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "the tool call forced a second step");
    assert!(
        !seen[0].iter().any(|text| text == "steer now"),
        "the request already in flight must not carry the steer: {:?}",
        seen[0]
    );
    assert_eq!(
        seen[1],
        vec!["start the turn".to_string(), "steer now".to_string()],
        "the next request carries it as a user turn, in order"
    );
    assert_eq!(
        tickets.lock().unwrap().as_slice(),
        &[7],
        "the sink is told the ticket at the point the step takes it"
    );

    let session = agent.session();
    let turns = user_turns(&session);
    assert_eq!(
        turns,
        vec!["start the turn".to_string(), "steer now".to_string()],
        "the session records the message at the point it was taken"
    );
    let final_text = session.messages().iter().find_map(|message| match message {
        react::LlmMessage::Assistant { content } => Some(content.clone()),
        _ => None,
    });
    assert_eq!(
        final_text.as_deref(),
        Some("acknowledged"),
        "the turn keeps streaming after the steer"
    );
}

/// A steer no step ever takes is handed back instead of vanishing with the
/// turn: the host restores it, which is the only honest outcome for a message
/// the model never saw.
struct SteerAfterLastStep {
    mailbox: Arc<SteerMailbox>,
    fired: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl AgentHook for SteerAfterLastStep {
    async fn on_event(&self, _event: HookEvent, _context: &HookContext) -> HookDecision {
        // OnComplete is raised once by the engine's final answer and once by
        // the Agent wrapper's own completion; only the first is the race under
        // test.
        if !self.fired.swap(true, std::sync::atomic::Ordering::SeqCst) {
            self.mailbox.push(SteerMessage {
                content: Content::Text("too late".to_string()),
                ticket: 9,
            });
        }
        HookDecision::Continue
    }
}

#[tokio::test]
async fn a_steer_the_turn_never_takes_is_withdrawn() {
    let provider = make_llm_provider();
    let agent = Agent::new(AgentConfig::default(), Arc::new(provider));
    let mailbox = agent.steers().clone();
    // OnComplete fires as the turn COMMITS its final answer - after the
    // next-step check has already passed - so this models a steer landing in
    // the closing instant. A hook earlier in the step would extend the turn
    // instead (see the extension test below).
    agent.hooks().register(
        HookEvent::OnComplete,
        Arc::new(SteerAfterLastStep {
            mailbox: Arc::clone(&mailbox),
            fired: std::sync::atomic::AtomicBool::new(false),
        }),
    );

    let stream = agent.stream("answer in one step");
    futures::pin_mut!(stream);
    while let Some(item) = stream.next().await {
        assert!(item.is_ok(), "the turn streams cleanly: {item:?}");
    }

    assert_eq!(
        agent.take_undelivered_steers(),
        vec![9],
        "the unread message is handed back, not silently dropped"
    );
    assert_eq!(agent.steers().depth(), 0, "and the mailbox is empty again");
    assert!(
        !user_turns(&agent.session())
            .iter()
            .any(|text| text == "too late"),
        "the model never saw it"
    );
}

/// A mock that never asks for a tool: every call is a plain answer, so the ONLY
/// way the turn can reach a second request is the next-step keep-open rule.
struct PlainAnswerLlm {
    calls: std::sync::atomic::AtomicUsize,
    seen: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
}

#[async_trait]
impl LlmClient<AgentSession, AgentReactContext> for PlainAnswerLlm {
    async fn complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        _session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<LlmResponse, LlmError> {
        Ok(make_text_response("blocking path unused".to_string()))
    }

    async fn stream_complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, LlmError> {
        self.seen.lock().unwrap().push(user_turns(session));
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let text = if call == 0 {
            "first answer"
        } else {
            "second answer"
        };
        Ok(Box::pin(futures::stream::iter(vec![
            Ok(StreamToken::Text(text.to_string())),
            Ok(StreamToken::Done),
        ])))
    }

    fn supports_tools(&self) -> bool {
        false
    }

    fn provider_name(&self) -> &'static str {
        "plain-answer-mock"
    }
}

/// A steer that lands while a NO-TOOL answer streams keeps the turn open: the
/// next step carries it, the model answers inside the SAME turn, and the turn
/// only ends after that answer.
///
/// This is the reference's rule - its loop checks `inbox.nextStep.length`
/// before closing a turn - and the observable is the mock's session at call
/// two, not a flag.
#[tokio::test]
async fn a_steer_extends_a_turn_whose_step_made_no_tool_call() {
    let seen = Arc::new(std::sync::Mutex::new(Vec::<Vec<String>>::new()));
    let tickets = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
    let mut provider = LlmProvider::new();
    provider.register_vendor(
        "mock".to_string(),
        Box::new(PlainAnswerLlm {
            calls: std::sync::atomic::AtomicUsize::new(0),
            seen: Arc::clone(&seen),
        }),
    );
    let agent = Agent::new(AgentConfig::default(), Arc::new(provider));
    let mailbox = agent.steers().clone();
    // AfterLlmCall fires once the first answer has streamed but BEFORE the
    // engine decides whether the turn is over.
    agent.hooks().register(
        HookEvent::AfterLlmCall,
        Arc::new(SteerOnFirstStep {
            mailbox: Arc::clone(&mailbox),
            fired: std::sync::atomic::AtomicBool::new(false),
        }),
    );
    let sink_tickets = Arc::clone(&tickets);
    agent.on_steered(move |ticket| sink_tickets.lock().unwrap().push(ticket));

    let stream = agent.stream("start the turn");
    futures::pin_mut!(stream);
    while let Some(item) = stream.next().await {
        assert!(item.is_ok(), "the turn streams cleanly: {item:?}");
    }

    let seen = seen.lock().unwrap();
    assert_eq!(
        seen.len(),
        2,
        "the steer kept the turn open for a second request: {seen:?}"
    );
    assert_eq!(seen[0], vec!["start the turn".to_string()]);
    assert_eq!(
        seen[1],
        vec!["start the turn".to_string(), "steer now".to_string()],
        "the extended request carries the steer as a user turn"
    );
    assert_eq!(
        tickets.lock().unwrap().as_slice(),
        &[7],
        "the sink reports the ticket at the take"
    );

    let session = agent.session();
    let transcript: Vec<(bool, String)> = session
        .messages()
        .iter()
        .filter_map(|message| match message {
            react::LlmMessage::User {
                content: Content::Text(text),
            } => Some((true, text.clone())),
            react::LlmMessage::Assistant { content } => Some((false, content.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        transcript,
        vec![
            (true, "start the turn".to_string()),
            (false, "first answer".to_string()),
            (true, "steer now".to_string()),
            (false, "second answer".to_string()),
        ],
        "the transcript keeps the order the turn really produced"
    );
}

/// The bound and the order: eight wait, the ninth is refused, and taking them
/// hands them back oldest first.
#[test]
fn the_steer_mailbox_is_bounded_and_first_in_first_out() {
    let mut provider = LlmProvider::new();
    provider.register_vendor("mock".to_string(), Box::new(MockLlm::new()));
    let agent = Agent::new(AgentConfig::default(), Arc::new(provider));
    for index in 0..MAX_STEER_DEPTH {
        assert!(
            agent.steer(format!("message {index}"), index as u64),
            "message {index} fits the bound"
        );
    }
    assert!(
        !agent.steer("one too many", 99),
        "the mailbox refuses the message past its bound"
    );
    assert_eq!(agent.steers().depth(), MAX_STEER_DEPTH);
    let tickets = agent.steers().withdraw();
    assert_eq!(
        tickets,
        (0..MAX_STEER_DEPTH as u64).collect::<Vec<_>>(),
        "withdraw returns the tickets oldest first"
    );
    assert_eq!(agent.steers().depth(), 0);
    assert!(agent.take_undelivered_steers().is_empty());
}

#[tokio::test]
async fn compact_now_refuses_a_summary_that_does_not_shrink_the_context() {
    let agent = agent_with_summary(&"x".repeat(50_000));
    four_messages(&agent);
    let signal = std::sync::atomic::AtomicBool::new(false);
    let error = agent
        .compact_now(0, &signal)
        .await
        .expect_err("a summary larger than its span is refused");
    assert!(matches!(
        error,
        AgentError::Compaction(CompactionFailure::NotSmaller { .. })
    ));
    assert!(error
        .to_string()
        .contains("not smaller than the shadowed content"));
    assert_eq!(
        agent.session().messages().len(),
        4,
        "a refused summary leaves the history untouched"
    );
}
