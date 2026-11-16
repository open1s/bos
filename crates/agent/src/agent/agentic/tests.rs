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

/// The inject event must surface the WHOLE system prompt a turn sends, not just
/// the configured persona: the recalled memory is key info the model receives,
/// so it must not be missing from the injection the reader sees.
#[tokio::test]
async fn injected_system_prompt_includes_recalled_memory() {
    let memory = crate::memory::InMemoryMemory::new();
    memory
        .add("the deployment target is staging-cluster".to_string(), None)
        .await;
    let config = AgentConfig {
        system_prompt: "CUSTOM PERSONA".to_string(),
        ..AgentConfig::default()
    };
    let agent = Agent::new(config, Arc::new(LlmProvider::new())).with_memory(Arc::new(memory));
    let full = agent
        .injected_system_prompt(&react::llm::Content::text("where is the deployment target"))
        .await;
    assert!(full.contains("CUSTOM PERSONA"), "the persona is present");
    assert!(
        full.contains("Relevant memory:"),
        "the recalled memory block is present: {full}"
    );
    assert!(
        full.contains("staging-cluster"),
        "the memory content is present: {full}"
    );
    assert!(
        full.len() > "CUSTOM PERSONA".len(),
        "the injected prompt is more than the persona alone"
    );
}

/// A memory change invalidates the cached assembly, so the next turn's prompt
/// recalls the new note rather than a stale cached copy.
#[tokio::test]
async fn injected_system_prompt_reflects_memory_changes() {
    let memory = crate::memory::InMemoryMemory::new();
    memory.add("alpha fact".to_string(), None).await;
    let config = AgentConfig {
        system_prompt: "PERSONA".to_string(),
        ..AgentConfig::default()
    };
    let agent = Agent::new(config, Arc::new(LlmProvider::new())).with_memory(Arc::new(memory));
    let query = react::llm::Content::text("fact");
    let first = agent.injected_system_prompt(&query).await;
    assert!(
        first.contains("alpha fact"),
        "first assembly recalls the note"
    );
    agent.remember("beta fact".to_string()).await;
    let second = agent.injected_system_prompt(&query).await;
    assert!(
        second.contains("beta fact"),
        "the memory change is reflected, not a stale cache: {second}"
    );
}
/// Multimodal turns retrieve on their accompanying text; a media-only turn
/// skips retrieval rather than treating the payload as a query (issue #335).
#[tokio::test]
async fn multimodal_retrieval_uses_accompanying_text() {
    use react::llm::{Content, ContentPart};
    let memory = crate::memory::InMemoryMemory::new();
    memory
        .add("the deploy target is staging".to_string(), None)
        .await;
    let config = AgentConfig {
        system_prompt: "P".to_string(),
        ..AgentConfig::default()
    };
    let agent = Agent::new(config, Arc::new(LlmProvider::new())).with_memory(Arc::new(memory));

    // Text-only: retrieves.
    let text = Content::text("where is the deploy target");
    assert!(agent
        .injected_system_prompt(&text)
        .await
        .contains("staging"));

    // Mixed text + image: retrieves on the text part.
    let mixed = Content::parts(vec![
        ContentPart::Text {
            text: "where is the deploy target".to_string(),
        },
        ContentPart::Binary {
            binary: react::llm::Binary::from_url("image/png", "http://x/i.png", None),
        },
    ]);
    assert!(agent
        .injected_system_prompt(&mixed)
        .await
        .contains("staging"));

    // Image-only: no usable query, retrieval skipped, no memory injected.
    let image_only = Content::parts(vec![ContentPart::Binary {
        binary: react::llm::Binary::from_url("image/png", "http://x/i.png", None),
    }]);
    assert!(!agent
        .injected_system_prompt(&image_only)
        .await
        .contains("staging"));

    // Audio-only: likewise skipped.
    let audio_only = Content::audio("QUJD", "wav");
    assert!(!agent
        .injected_system_prompt(&audio_only)
        .await
        .contains("staging"));
}

/// retrieval_text extracts text parts and never reads binary as text.
#[test]
fn retrieval_text_scopes_to_text_parts() {
    use react::llm::{Content, ContentPart};
    assert_eq!(
        Content::text("hello").retrieval_text().as_deref(),
        Some("hello")
    );
    let mixed = Content::parts(vec![
        ContentPart::Text {
            text: "one".to_string(),
        },
        ContentPart::Binary {
            binary: react::llm::Binary::from_url("image/png", "http://x/i.png", None),
        },
        ContentPart::Text {
            text: "two".to_string(),
        },
    ]);
    assert_eq!(mixed.retrieval_text().as_deref(), Some("one two"));
    let image_only = Content::image("http://x/i.png");
    assert_eq!(image_only.retrieval_text(), None);
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

/// The request's tool array is NAME-SORTED, whatever order the tools were
/// registered in.
///
/// The registry is a concurrent map, and a shell rebuilds its agent every
/// turn: a fresh map walks in a fresh order, so the schemas used to arrive in
/// a different arrangement on every turn start. The tool array is part of the
/// provider's cached prompt prefix, so that reshuffle re-keyed the cache and
/// every turn re-prefilled the whole request.
#[test]
fn prepare_context_lists_tools_in_name_order() {
    let provider = {
        let mut provider = LlmProvider::new();
        provider.register_vendor(
            "toolmock".to_string(),
            Box::new(ToolCallLlm {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
        );
        provider
    };
    let mut agent = Agent::new(AgentConfig::default(), Arc::new(provider));
    for name in ["zulu_tool", "alpha_tool", "mike_tool", "bravo_tool"] {
        let owned = name.to_string();
        agent.add_tool(Arc::new(FunctionTool::new(
            name,
            "a registered tool",
            serde_json::json!({"type":"object"}),
            move |_args| Ok(serde_json::json!(owned.clone())),
        )));
    }
    let names: Vec<String> = agent
        .prepare_context()
        .tools
        .into_iter()
        .map(|tool| tool.name)
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "the tool array is name-sorted: {names:?}");
}

/// An async tool, the shape the extension platform installs: the host adds it
/// to the agent's live overlay *after* the build.
struct LiveEchoTool {
    name: &'static str,
    schema: serde_json::Value,
}

#[async_trait]
impl react::tool::registry::AsyncTool for LiveEchoTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> String {
        "echo the arguments".to_string()
    }

    fn json_schema(&self) -> serde_json::Value {
        self.schema.clone()
    }

    async fn run(
        &self,
        arguments: &serde_json::Value,
    ) -> Result<serde_json::Value, react::tool::ToolError> {
        Ok(arguments.clone())
    }
}

/// A package's tool reaches the model only through the request's tool array.
///
/// The live overlay is what BSH's extension platform registers into, and the
/// engine executes those tools either way - so a missing array entry is silent:
/// the tool exists, runs when called, and is never called, because the model
/// was never told it exists. The array, not the engine registry, is the
/// contract with the model.
#[test]
fn prepare_context_advertises_live_extension_tools() {
    let agent = Agent::new(AgentConfig::default(), Arc::new(make_llm_provider()));
    let schema = serde_json::json!({
        "type": "object",
        "properties": {"text": {"type": "string"}},
        "required": ["text"]
    });
    agent
        .add_live_async_tool(Arc::new(LiveEchoTool {
            name: "com.example.echo",
            schema: schema.clone(),
        }))
        .expect("the live tool registers");

    let tools = agent.prepare_context().tools;
    let echo = tools
        .iter()
        .find(|tool| tool.name == "com.example.echo")
        .expect("a live extension tool must be advertised to the model");
    assert_eq!(
        echo.parameters, schema,
        "the model needs the tool's own schema, not an empty object"
    );
}

/// Registry and live overlay share one model-facing namespace: a name owned by
/// both is advertised once, so the provider is not handed a duplicate entry.
#[test]
fn prepare_context_advertises_a_live_tool_once() {
    let mut agent = Agent::new(AgentConfig::default(), Arc::new(make_llm_provider()));
    agent.add_tool(Arc::new(FunctionTool::new(
        "echo",
        "a registered tool",
        serde_json::json!({"type":"object"}),
        |_args| Ok(serde_json::json!("registered")),
    )));
    agent
        .add_live_async_tool(Arc::new(LiveEchoTool {
            name: "echo",
            schema: serde_json::json!({"type":"object"}),
        }))
        .expect("the live tool registers");

    let count = agent
        .prepare_context()
        .tools
        .iter()
        .filter(|tool| tool.name == "echo")
        .count();
    assert_eq!(count, 1, "one name, one array entry");
}

/// Withholding a tool removes its schema from every step but keeps it
/// callable, and the pair that replaces it is advertised in its place.
#[test]
fn a_deferred_tool_leaves_the_array_and_the_discovery_pair_takes_its_place() {
    let mut agent = Agent::new(AgentConfig::default(), Arc::new(make_llm_provider()));
    agent.add_tool(Arc::new(FunctionTool::new(
        "hidden",
        "a tool the prompt does not name",
        serde_json::json!({"type": "object"}),
        |_args| Ok(serde_json::json!("hidden")),
    )));
    let agent = agent.defer_tool("hidden");

    assert!(agent.has_deferred_tools());
    assert!(agent.is_deferred_tool("hidden"));
    assert!(!agent.is_deferred_tool("visible"));

    let names: Vec<String> = agent
        .prepare_context()
        .tools
        .into_iter()
        .map(|tool| tool.name)
        .collect();
    assert!(
        !names.iter().any(|name| name == "hidden"),
        "a deferred tool is not advertised: {names:?}"
    );
    assert!(
        names.iter().any(|name| name == "find_tool"),
        "the search is advertised: {names:?}"
    );
}

/// The default agent withholds nothing, so it pays for no discovery pair.
#[test]
fn nothing_is_deferred_by_default() {
    let agent = Agent::new(AgentConfig::default(), Arc::new(make_llm_provider()));
    assert!(!agent.has_deferred_tools());
    let names: Vec<String> = agent
        .prepare_context()
        .tools
        .into_iter()
        .map(|tool| tool.name)
        .collect();
    assert!(
        !names.iter().any(|name| name == "find_tool"),
        "no withheld tool means no discovery search: {names:?}"
    );
}

// =========================================================================
// Initial tool-schema budget (issue #292)
// =========================================================================

/// Register `count` tools whose definitions are each about `bytes` long.
fn register_sized_tools(agent: &mut Agent, count: usize, bytes: usize) {
    for index in 0..count {
        let description: &'static str = Box::leak("x".repeat(bytes).into_boxed_str());
        let name: &'static str = Box::leak(format!("sized_{index:02}").into_boxed_str());
        agent.add_tool(Arc::new(FunctionTool::new(
            name,
            description,
            serde_json::json!({"type": "object"}),
            move |_args| Ok(serde_json::json!("ok")),
        )));
    }
}

/// The initial array is bounded by the configured budget, and every definition
/// it leaves out is named by the admission decision and reachable through the
/// search that replaces it.
#[test]
fn a_large_registry_cannot_exceed_the_initial_schema_budget() {
    let mut agent = Agent::new(AgentConfig::default(), Arc::new(make_llm_provider()));
    // 40 definitions of 2 KB each: 80 KB of schema against a 20 KB ceiling.
    register_sized_tools(&mut agent, 40, 2_048);
    let agent = agent.with_tool_schema_budget(20_000);

    let admission = agent.tool_admission();
    assert!(
        admission.bytes <= admission.limit,
        "the advertised array is bounded: {} bytes over a {} byte limit",
        admission.bytes,
        admission.limit
    );
    assert!(
        admission.has_withheld(),
        "the rest of the registry is withheld, not silently shipped"
    );
    // `find_tool` is admitted in place of the withheld definitions, so the
    // accounting is over the REGISTRY: every registered definition is either
    // advertised or reported, and the search is the extra entry.
    let advertised: std::collections::HashSet<&str> = admission
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    let withheld: std::collections::HashSet<&str> = admission
        .withheld
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    for tool in agent.all_tools() {
        assert!(
            advertised.contains(tool.name.as_str()) || withheld.contains(tool.name.as_str()),
            "{} is neither advertised nor reported",
            tool.name
        );
    }
    assert!(
        advertised.contains("find_tool"),
        "the search is admitted in the withheld definitions' place"
    );

    let names: Vec<String> = agent.model_tools().into_iter().map(|t| t.name).collect();
    for withheld in admission.withheld_names() {
        assert!(
            !names.contains(&withheld),
            "{withheld} is withheld, so it must not be in the array: {names:?}"
        );
    }
    assert!(
        names.iter().any(|name| name == "find_tool"),
        "the way back to a withheld definition is advertised: {names:?}"
    );
}

/// A definition the prompt names is never withheld, even when the budget
/// cannot afford it: the required bytes are admitted and the overdraft is
/// reported instead of being paid for by dropping the tool.
#[test]
fn a_required_tool_is_admitted_even_when_the_budget_is_too_small() {
    let mut agent = Agent::new(AgentConfig::default(), Arc::new(make_llm_provider()));
    let required: &'static str = Box::leak("r".repeat(20_000).into_boxed_str());
    agent.add_tool(Arc::new(FunctionTool::new(
        "read_file",
        required,
        serde_json::json!({"type": "object"}),
        |_args| Ok(serde_json::json!("ok")),
    )));
    register_sized_tools(&mut agent, 4, 2_048);
    let mut agent = agent.with_tool_schema_budget(8_000);
    agent.require_tool("read_file");

    let admission = agent.tool_admission();
    assert!(
        admission.tools.iter().any(|tool| tool.name == "read_file"),
        "a required definition stays in the array"
    );
    assert!(admission.over_budget, "the overdraft is reported");
    assert_eq!(agent.required_tools(), &["read_file".to_string()][..]);
}

/// Withholding a definition for budget keeps it callable: the search the
/// request carries reports the full definition, so a direct call can follow.
#[test]
fn a_budget_withheld_tool_is_found_by_the_search() {
    let mut agent = Agent::new(AgentConfig::default(), Arc::new(make_llm_provider()));
    register_sized_tools(&mut agent, 12, 2_048);
    let agent = agent.with_tool_schema_budget(16_000);

    let admission = agent.tool_admission();
    let withheld = admission
        .withheld_names()
        .into_iter()
        .next()
        .expect("a 12 x 2 KB registry does not fit 16 KB");

    let search = super::discovery::find_tool(
        agent.registry().cloned(),
        Arc::new(std::sync::RwLock::new(ToolRegistry::new())),
        Arc::new(std::sync::RwLock::new(None)),
        super::discovery::Withheld::new(Vec::new(), Vec::new())
            .with_budget(Arc::clone(&agent.schema_withheld)),
    );
    let reported = search
        .run(&serde_json::json!({"name": withheld}))
        .expect("the search answers");
    assert_eq!(
        reported.get("name").and_then(|name| name.as_str()),
        Some(withheld.as_str()),
        "the withheld definition is reported instead of 'already in your tool list': {reported}"
    );
    assert!(
        reported.get("parameters").is_some(),
        "the result carries the schema a direct call needs"
    );
}

/// A zero configuration is not "unlimited": it selects the framework ceiling,
/// which is the same number discovery already spends.
#[test]
fn a_zero_schema_budget_selects_the_framework_ceiling() {
    let mut agent = Agent::new(AgentConfig::default(), Arc::new(make_llm_provider()));
    register_sized_tools(&mut agent, 80, 2_048);
    let agent = agent.with_tool_schema_budget(0);

    let admission = agent.tool_admission();
    assert_eq!(agent.tool_schema_budget(), 0);
    assert_eq!(
        admission.limit,
        react::engine::DEFAULT_SCHEMA_BUDGET - react::tool::admission::DEFAULT_DISCOVERY_RESERVE
    );
    assert!(
        admission.bytes <= admission.limit,
        "the default ceiling bounds the initial array"
    );
    assert!(admission.has_withheld(), "160 KB does not fit the default");
}

/// The cached prefix cannot depend on registration order: the same set yields
/// the same array, whatever order the registry walks in.
#[test]
fn the_admitted_array_is_stable_across_registration_order() {
    let names_of = |registration: &[&str]| {
        let mut agent = Agent::new(AgentConfig::default(), Arc::new(make_llm_provider()));
        for name in registration {
            let owned = name.to_string();
            agent.add_tool(Arc::new(FunctionTool::new(
                name,
                "a tool",
                serde_json::json!({"type": "object"}),
                move |_args| Ok(serde_json::json!(owned.clone())),
            )));
        }
        let agent = agent.with_tool_schema_budget(10_000);
        agent
            .model_tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<String>>()
    };
    assert_eq!(
        names_of(&["zulu", "alpha", "mike", "bravo"]),
        names_of(&["bravo", "mike", "alpha", "zulu"])
    );
}

// =========================================================================
// Action-first turns (issue #293)
// =========================================================================

/// One scripted model step.
enum Step {
    /// A prose reply with no tool call.
    Text(&'static str),
    /// One tool call.
    Tool(&'static str, &'static str),
}

/// A model that plays a fixed script and records the user messages the engine
/// appends, so a test can see the corrective action note rather than infer it.
struct ActionLlm {
    step: std::sync::atomic::AtomicUsize,
    script: Vec<Step>,
    seen: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
}

impl ActionLlm {
    fn next(&self, session: &AgentSession) -> Option<&Step> {
        self.seen.lock().unwrap().push(user_turns(session));
        let step = self.step.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.script.get(step)
    }
}

#[async_trait]
impl LlmClient<AgentSession, AgentReactContext> for ActionLlm {
    async fn complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<LlmResponse, LlmError> {
        Ok(match self.next(session) {
            Some(Step::Text(text)) => make_text_response((*text).to_string()),
            Some(Step::Tool(name, args)) => make_tool_call_response(name, args),
            None => make_text_response("Final Answer: script exhausted".to_string()),
        })
    }

    async fn stream_complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, LlmError> {
        let tokens: Vec<Result<StreamToken, LlmError>> = match self.next(session) {
            Some(Step::Text(text)) => vec![
                Ok(StreamToken::Text((*text).to_string())),
                Ok(StreamToken::Done),
            ],
            Some(Step::Tool(name, args)) => vec![
                Ok(StreamToken::ToolCall {
                    name: (*name).to_string(),
                    args: serde_json::from_str(args).unwrap_or(serde_json::json!({})),
                    id: Some(format!(
                        "action-{}",
                        self.step.load(std::sync::atomic::Ordering::SeqCst)
                    )),
                }),
                Ok(StreamToken::Done),
            ],
            None => vec![
                Ok(StreamToken::Text("script exhausted".to_string())),
                Ok(StreamToken::Done),
            ],
        };
        Ok(Box::pin(futures::stream::iter(tokens)))
    }

    fn supports_tools(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "action-mock"
    }
}

/// An agent with the scripted model and, when asked, one workspace-shaped tool.
fn action_agent(
    script: Vec<Step>,
    with_tool: bool,
) -> (Agent, Arc<std::sync::Mutex<Vec<Vec<String>>>>) {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut provider = LlmProvider::new();
    provider.register_vendor(
        "mock".to_string(),
        Box::new(ActionLlm {
            step: std::sync::atomic::AtomicUsize::new(0),
            script,
            seen: seen.clone(),
        }),
    );
    let mut agent = Agent::new(AgentConfig::default(), Arc::new(provider));
    if with_tool {
        agent.add_tool(Arc::new(FunctionTool::new(
            "read_file",
            "read a file",
            serde_json::json!({"type": "object"}),
            |_args| Ok(serde_json::json!({"content": "ok"})),
        )));
    }
    (agent, seen)
}

/// How many action notes the LAST model call saw in its session.
///
/// The mock records the session at every call, so this reads what the engine
/// actually sent rather than the post-run transcript.
fn action_notes(seen: &Arc<std::sync::Mutex<Vec<Vec<String>>>>) -> usize {
    seen.lock()
        .unwrap()
        .last()
        .map(|turns| {
            turns
                .iter()
                .filter(|text| text.as_str() == react::engine::ACTION_NOTE)
                .count()
        })
        .unwrap_or(0)
}

/// An actionable turn answered with prose only is sent back to work once: the
/// next step calls a tool, and the turn ends with the work done.
#[tokio::test]
async fn an_action_turn_is_sent_back_to_work_after_a_prose_only_reply() {
    let (agent, seen) = action_agent(
        vec![
            Step::Text("Here is how I would fix it: open the file and change the guard."),
            Step::Tool("read_file", "{}"),
            Step::Text("Final Answer: fixed the guard and read the file to verify."),
        ],
        true,
    );
    agent.set_action_intent(ActionIntent::Action);
    let answer = agent
        .react("fix the failing guard")
        .await
        .expect("the turn");

    assert!(
        answer.contains("fixed the guard"),
        "the real answer closes the turn: {answer}"
    );
    assert_eq!(
        action_notes(&seen),
        1,
        "the engine asked once for work, not a plan"
    );
    assert_eq!(
        seen.lock().unwrap().len(),
        3,
        "step two acts, step three reports"
    );
}

/// The nudge is spent once: a model that keeps explaining is allowed to close
/// the turn with its second answer rather than looping.
#[tokio::test]
async fn the_action_nudge_is_spent_once() {
    let (agent, seen) = action_agent(
        vec![
            Step::Text("I would start by reading the parser."),
            Step::Text("Final Answer: still a plan."),
            Step::Text("Final Answer: never reached."),
        ],
        true,
    );
    agent.set_action_intent(ActionIntent::Action);
    let answer = agent.react("fix the parser").await.expect("the turn");

    assert!(
        answer.contains("still a plan"),
        "the second answer closes the turn: {answer}"
    );
    assert_eq!(seen.lock().unwrap().len(), 2, "one retry, not a loop");
    assert_eq!(action_notes(&seen), 1, "the note was sent exactly once");
}

/// An explanation turn never pays for the action policy: prose is the
/// deliverable, so a talk-only reply is the answer.
#[tokio::test]
async fn an_explanation_turn_is_not_nudged() {
    let (agent, seen) = action_agent(vec![Step::Text("Final Answer: it works by caching.")], true);
    agent.set_action_intent(ActionIntent::Explanation);
    let answer = agent
        .react("explain how the cache works")
        .await
        .expect("the turn");

    assert!(answer.contains("caching"), "{answer}");
    assert_eq!(action_notes(&seen), 0);
    assert_eq!(seen.lock().unwrap().len(), 1, "one request, no retry");
}

/// A turn with no callable workspace tool cannot act, so the engine does not
/// spend a retry asking it to.
#[tokio::test]
async fn an_action_turn_without_tools_is_not_nudged() {
    let (agent, seen) = action_agent(
        vec![Step::Text("Final Answer: no tool is available.")],
        false,
    );
    agent.set_action_intent(ActionIntent::Action);
    let answer = agent.react("fix the guard").await.expect("the turn");

    assert!(answer.contains("no tool is available"), "{answer}");
    assert_eq!(action_notes(&seen), 0);
}

/// A reply that ends in a question is the contract's "ask once when the request
/// is too vague" arm, not a dodge.
#[tokio::test]
async fn a_question_reply_to_an_action_turn_is_not_nudged() {
    let (agent, seen) = action_agent(vec![Step::Text("Which file should I change?")], true);
    agent.set_action_intent(ActionIntent::Action);
    let answer = agent.react("make it better").await.expect("the turn");

    assert!(answer.contains("Which file"), "{answer}");
    assert_eq!(action_notes(&seen), 0);
}

/// The streaming path applies the same contract: a prose-only step is held open
/// with the note, and the next step's tool call runs.
#[tokio::test]
async fn an_action_turn_is_sent_back_to_work_on_the_streaming_path() {
    use futures::StreamExt;

    let (agent, seen) = action_agent(
        vec![
            Step::Text("I would inspect the file first."),
            Step::Tool("read_file", "{}"),
            Step::Text("Final Answer: inspected it."),
        ],
        true,
    );
    agent.set_action_intent(ActionIntent::Action);

    let mut stream = agent.stream("fix the guard");
    let mut answer = String::new();
    while let Some(item) = stream.next().await {
        if let Ok(StreamToken::Text(text)) = item {
            answer.push_str(&text);
        }
    }
    assert!(answer.contains("inspected it"), "{answer}");
    assert_eq!(action_notes(&seen), 1);
    assert_eq!(seen.lock().unwrap().len(), 3);
}

/// A classifier vendor: answers ACTION and counts how many requests it served.
struct ClassifyLlm {
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl LlmClient<AgentSession, AgentReactContext> for ClassifyLlm {
    async fn complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        _session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<LlmResponse, LlmError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(make_text_response("ACTION".to_string()))
    }

    async fn stream_complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        _session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, LlmError> {
        Ok(Box::pin(futures::stream::empty()))
    }

    fn supports_tools(&self) -> bool {
        false
    }

    fn provider_name(&self) -> &'static str {
        "classify-mock"
    }
}

/// A vendor whose classifier request fails.
struct OfflineLlm;

#[async_trait]
impl LlmClient<AgentSession, AgentReactContext> for OfflineLlm {
    async fn complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        _session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<LlmResponse, LlmError> {
        Err(LlmError::Other("offline".to_string()))
    }

    async fn stream_complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        _session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, LlmError> {
        Ok(Box::pin(futures::stream::empty()))
    }

    fn supports_tools(&self) -> bool {
        false
    }

    fn provider_name(&self) -> &'static str {
        "offline-mock"
    }
}

/// The semantic second opinion answers an unknown turn, and a repeat of the
/// same message is served from the cache instead of paying another completion.
#[tokio::test]
async fn the_model_classifier_answers_unknown_turns_and_caches() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut provider = LlmProvider::new();
    provider.register_vendor(
        "mock".to_string(),
        Box::new(ClassifyLlm {
            calls: calls.clone(),
        }),
    );
    let agent = Agent::new(AgentConfig::default(), Arc::new(provider));

    assert_eq!(
        agent
            .classify_intent_with_model("the parser gives a confusing error")
            .await,
        Some(ActionIntent::Action)
    );
    assert_eq!(
        agent
            .classify_intent_with_model("the parser gives a confusing error")
            .await,
        Some(ActionIntent::Action)
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the second identical message is cached"
    );
    let usage = agent
        .take_auxiliary_usage()
        .expect("the classifier's usage is accounted");
    assert_eq!(usage.prompt_tokens, 10, "the mock's prompt tokens");
    assert_eq!(usage.completion_tokens, 20, "the mock's completion tokens");
    assert!(
        agent.take_auxiliary_usage().is_none(),
        "draining is one-shot"
    );
}

/// A classifier failure is not a turn failure: the caller keeps its lexical
/// answer instead of losing the message to an unavailable vendor.
#[tokio::test]
async fn a_classifier_failure_keeps_the_lexical_answer() {
    let mut provider = LlmProvider::new();
    provider.register_vendor("mock".to_string(), Box::new(OfflineLlm));
    let agent = Agent::new(AgentConfig::default(), Arc::new(provider));
    assert_eq!(
        agent
            .classify_intent_with_model("the parser gives a confusing error")
            .await,
        None
    );
}

/// An agent with the scripted model and one tool that always fails.
fn action_agent_with_failing_tool(
    script: Vec<Step>,
) -> (Agent, Arc<std::sync::Mutex<Vec<Vec<String>>>>) {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut provider = LlmProvider::new();
    provider.register_vendor(
        "mock".to_string(),
        Box::new(ActionLlm {
            step: std::sync::atomic::AtomicUsize::new(0),
            script,
            seen: seen.clone(),
        }),
    );
    let mut agent = Agent::new(AgentConfig::default(), Arc::new(provider));
    agent.add_tool(Arc::new(FunctionTool::new(
        "read_file",
        "read a file",
        serde_json::json!({"type": "object"}),
        |_args| {
            Err::<serde_json::Value, react::tool::ToolError>(react::tool::ToolError::Failed(
                "permission denied".to_string(),
            ))
        },
    )));
    (agent, seen)
}

/// An unclassified turn that promises to act instead of acting still gets the
/// one retry: the classifier is conservative, but the talk-only shape is not.
#[tokio::test]
async fn an_unclassified_turn_that_promises_to_act_is_nudged() {
    let (agent, seen) = action_agent(
        vec![
            Step::Text("I will start by reading the parser."),
            Step::Tool("read_file", "{}"),
            Step::Text("Final Answer: read the parser."),
        ],
        true,
    );
    // Unknown is the default; the promise is what earns the retry.
    agent.set_action_intent(ActionIntent::Unknown);
    let answer = agent
        .react("the parser is confusing")
        .await
        .expect("the turn");

    assert!(answer.contains("read the parser"), "{answer}");
    assert_eq!(action_notes(&seen), 1);
    assert_eq!(seen.lock().unwrap().len(), 3);
}

/// The promise guard does not override an explicit explanation request: an
/// explanation may say "first, I will explain..." and must not be sent to act.
#[tokio::test]
async fn an_explanation_turn_that_promises_to_explain_is_not_nudged() {
    let (agent, seen) = action_agent(
        vec![Step::Text(
            "First, I will explain how the cache works: it stores...",
        )],
        true,
    );
    agent.set_action_intent(ActionIntent::Explanation);
    let answer = agent.react("explain the cache").await.expect("the turn");

    assert!(answer.contains("stores"), "{answer}");
    assert_eq!(action_notes(&seen), 0);
    assert_eq!(seen.lock().unwrap().len(), 1);
}

/// When an ACTION turn attempted work and every call failed, the engine refuses
/// a completion claim and asks for the exact failure instead - once.
#[tokio::test]
async fn a_failed_action_attempt_is_sent_back_to_report_the_failure() {
    let (agent, seen) = action_agent_with_failing_tool(vec![
        Step::Tool("read_file", "{}"),
        Step::Text("Final Answer: done, the file is fixed."),
        Step::Text("Final Answer: the read was refused with permission denied."),
    ]);
    agent.set_action_intent(ActionIntent::Action);
    let answer = agent.react("fix the parser").await.expect("the turn");

    assert!(
        answer.contains("permission denied"),
        "the honest failure report closes the turn: {answer}"
    );
    assert_eq!(seen.lock().unwrap().len(), 3, "one failure retry");
    let notes = seen.lock().unwrap().last().cloned().unwrap_or_default();
    assert!(
        notes
            .iter()
            .any(|text| text == react::engine::ACTION_FAILED_NOTE),
        "the failed-attempt note was sent: {notes:?}"
    );
}

// =========================================================================
// Transient read-only tool retry
// =========================================================================

/// A read-only tool that fails once with a transient error.
fn flaky_tool(
    name: &'static str,
    effects: &[&str],
    calls: Arc<std::sync::atomic::AtomicUsize>,
) -> FunctionTool {
    let metadata = react::tool::ToolMetadata::new()
        .with_side_effects(effects.iter().map(|effect| effect.to_string()));
    FunctionTool::new(
        name,
        "a probe",
        serde_json::json!({"type": "object"}),
        move |_args| {
            let call = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if call == 0 {
                Err(react::tool::ToolError::Failed(
                    "connection reset by peer".to_string(),
                ))
            } else {
                Ok(serde_json::json!({"ok": true}))
            }
        },
    )
    .with_metadata(metadata)
}

/// A transient failure of a tool that declares read-only side effects is
/// retried once: the environment was briefly unavailable, and repeating a read
/// cannot duplicate a side effect.
#[tokio::test]
async fn a_transient_read_only_failure_is_retried_once() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (mut agent, seen) = action_agent(
        vec![
            Step::Tool("probe", "{}"),
            Step::Text("Final Answer: read it."),
        ],
        false,
    );
    agent.add_tool(Arc::new(flaky_tool("probe", &["read"], calls.clone())));
    agent.set_action_intent(ActionIntent::Action);
    let answer = agent.react("inspect the probe").await.expect("the turn");

    assert!(answer.contains("read it"), "{answer}");
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "one transient retry ran the tool again"
    );
    assert_eq!(
        action_notes(&seen),
        0,
        "the retry did not need a model nudge"
    );
}

/// A tool that declares a write is never retried by the harness: a second edit
/// is a second edit, whatever the failure text says.
#[tokio::test]
async fn a_declared_write_tool_is_not_retried() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (mut agent, _seen) = action_agent(
        vec![
            Step::Tool("probe", "{}"),
            Step::Text("Final Answer: failed."),
        ],
        false,
    );
    agent.add_tool(Arc::new(flaky_tool("probe", &["write"], calls.clone())));
    agent.set_action_intent(ActionIntent::Unknown);
    let answer = agent.react("edit the probe").await.expect("the turn");

    assert!(answer.contains("failed"), "{answer}");
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a write is not replayed"
    );
}

// =========================================================================
// Deferred tools end to end
// =========================================================================

/// An MCP-shaped async tool that records that it ran.
struct CountingMcpTool {
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl react::tool::registry::AsyncTool for CountingMcpTool {
    fn name(&self) -> &str {
        "github_search"
    }

    fn description(&self) -> String {
        "Search GitHub issues in the connected repository".to_string()
    }

    fn json_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {"query": {"type": "string", "description": "Search text"}},
            "required": ["query"]
        })
    }

    async fn run(
        &self,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, react::tool::ToolError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(serde_json::json!({
            "issues": [{"title": format!("found for {}", args["query"].as_str().unwrap_or(""))}]
        }))
    }
}

/// Records the name each before-tool hook was given.
struct ToolNameHook {
    names: Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait]
impl AgentHook for ToolNameHook {
    async fn on_event(&self, _event: HookEvent, context: &HookContext) -> HookDecision {
        if let Some(name) = context.get("tool_name") {
            self.names.lock().unwrap().push(name.clone());
        }
        HookDecision::Continue
    }
}

/// A model that discovers a withheld tool through `find_tool`, then calls it
/// directly - the whole round trip, through the real agent loop.
struct DiscoveringLlm {
    step: std::sync::atomic::AtomicUsize,
    advertised: Arc<std::sync::Mutex<Vec<String>>>,
    /// The `find_tool` arguments this model sends on its first step.
    find_args: String,
    /// The tool it calls directly once the search reports it.
    direct: String,
}

#[async_trait]
impl LlmClient<AgentSession, AgentReactContext> for DiscoveringLlm {
    async fn complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        _session: &mut AgentSession,
        context: &mut AgentReactContext,
    ) -> Result<LlmResponse, LlmError> {
        let step = self.step.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // The discovery result must join the NEXT request's tool array; that is
        // the step where the model calls the reported tool directly.
        if step == 1 {
            let names = context.tools.iter().map(|tool| tool.name.clone()).collect();
            *self.advertised.lock().unwrap() = names;
        }
        Ok(match step {
            0 => make_tool_call_response("find_tool", &self.find_args),
            1 => make_tool_call_response(&self.direct, "{\"query\":\"bug\"}"),
            _ => make_text_response("Final Answer: found the issues".to_string()),
        })
    }

    async fn stream_complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        _session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<react::llm::TokenStream, LlmError> {
        Ok(Box::pin(futures::stream::empty()))
    }

    fn supports_tools(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "discovering-mock"
    }
}

fn discovering_agent(
    calls: Arc<std::sync::atomic::AtomicUsize>,
    advertised: Arc<std::sync::Mutex<Vec<String>>>,
) -> Agent {
    discovering_agent_for(
        calls,
        advertised,
        "{\"query\":\"github issue search\"}",
        "github_search",
    )
}

/// The same script, with the search arguments and the tool it reports chosen by
/// the caller.
fn discovering_agent_for(
    calls: Arc<std::sync::atomic::AtomicUsize>,
    advertised: Arc<std::sync::Mutex<Vec<String>>>,
    find_args: &str,
    direct: &str,
) -> Agent {
    let mut provider = LlmProvider::new();
    provider.register_vendor(
        "mock".to_string(),
        Box::new(DiscoveringLlm {
            step: std::sync::atomic::AtomicUsize::new(0),
            advertised,
            find_args: find_args.to_string(),
            direct: direct.to_string(),
        }),
    );
    let mut agent = Agent::new(AgentConfig::default(), Arc::new(provider));
    agent
        .add_mcp_tool("github", "search", Arc::new(CountingMcpTool { calls }))
        .expect("the MCP tool registers");
    agent
}

/// An MCP tool is callable, and unreachable without the pair: the model asks
/// `find_tool`, calls the reported answer directly, and the hooks and the tool
/// itself see one real invocation - not a dispatcher and a victim.
#[tokio::test]
async fn a_withheld_mcp_tool_is_found_and_run_through_the_agent() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let advertised = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut agent = discovering_agent(calls.clone(), advertised.clone());
    let hook_names = Arc::new(std::sync::Mutex::new(Vec::new()));
    agent.add_hook(
        HookEvent::BeforeToolCall,
        Arc::new(ToolNameHook {
            names: hook_names.clone(),
        }),
    );

    let answer = agent
        .react("find the open bugs".to_string())
        .await
        .expect("the turn completes");

    assert!(
        answer.contains("found the issues"),
        "the model's answer came back: {answer}"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the MCP tool ran exactly once"
    );
    let seen = advertised.lock().unwrap().clone();
    assert!(
        seen.iter().any(|name| name == "find_tool")
            && seen.iter().any(|name| name == "github_search"),
        "the search and the discovered tool are both advertised: {seen:?}"
    );
    assert_eq!(
        *hook_names.lock().unwrap(),
        vec!["find_tool".to_string(), "github_search".to_string()],
        "the second hook sees the real discovered tool"
    );
}

/// A definition withheld by the BYTE BUDGET is reachable the same way a
/// deferred one is: the search the budget admitted reports it, the reported
/// schema joins the next request, and the direct call runs.
///
/// The fixture's first withheld name is an assumption about the registry's
/// sizes; the test asserts it, so a change in the definition size fails here
/// with the reason instead of turning this into a green no-op.
#[tokio::test]
async fn a_budget_withheld_tool_is_found_and_run_after_the_initial_set_is_bounded() {
    let direct = "sized_03";
    let advertised = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut provider = LlmProvider::new();
    provider.register_vendor(
        "mock".to_string(),
        Box::new(DiscoveringLlm {
            step: std::sync::atomic::AtomicUsize::new(0),
            advertised: advertised.clone(),
            find_args: format!("{{\"name\":\"{direct}\"}}"),
            direct: direct.to_string(),
        }),
    );
    let mut agent = Agent::new(AgentConfig::default(), Arc::new(provider));
    register_sized_tools(&mut agent, 12, 2_048);
    let agent = agent.with_tool_schema_budget(16_000);

    let admission = agent.tool_admission();
    assert_eq!(
        admission.withheld_names().first().map(String::as_str),
        Some(direct),
        "the fixture's direct call names the first withheld definition"
    );
    assert!(
        !admission.tools.iter().any(|tool| tool.name == direct),
        "the premise: this definition is not in the initial request"
    );

    let answer = agent
        .react("use the sized tool".to_string())
        .await
        .expect("the turn completes");
    assert!(
        answer.contains("found the issues"),
        "the turn ran the discovered tool and answered: {answer}"
    );
    let seen = advertised.lock().unwrap().clone();
    assert!(
        seen.iter().any(|name| name == "find_tool") && seen.iter().any(|name| name == direct),
        "the search and the budget-withheld tool are both advertised after discovery: {seen:?}"
    );
}

/// A scripted model that names the withheld tool directly.
struct DirectLlm {
    step: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl LlmClient<AgentSession, AgentReactContext> for DirectLlm {
    async fn complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        _session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<LlmResponse, LlmError> {
        let step = self.step.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(match step {
            0 => make_tool_call_response("github_search", "{\"query\":\"bug\"}"),
            _ => make_text_response("Final Answer: done".to_string()),
        })
    }

    async fn stream_complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        _session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<react::llm::TokenStream, LlmError> {
        Ok(Box::pin(futures::stream::empty()))
    }

    fn supports_tools(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "direct-mock"
    }
}

/// Withholding is advertisement, not permission: a model that names a withheld
/// tool still reaches it, which is what makes the pair a convenience rather
/// than a gate.
#[tokio::test]
async fn a_withheld_tool_still_runs_when_named_directly() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut provider = LlmProvider::new();
    provider.register_vendor(
        "mock".to_string(),
        Box::new(DirectLlm {
            step: std::sync::atomic::AtomicUsize::new(0),
        }),
    );
    let mut agent = Agent::new(AgentConfig::default(), Arc::new(provider));
    agent
        .add_mcp_tool(
            "github",
            "search",
            Arc::new(CountingMcpTool {
                calls: calls.clone(),
            }),
        )
        .expect("the MCP tool registers");

    let _ = agent.react("bugs".to_string()).await.expect("turn");
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a withheld tool is still registered and callable"
    );
}

/// A model that finds a skill, loads it, and reports whether the instructions
/// arrived - the search path end to end, through the real agent loop.
struct SkillSearchLlm {
    step: std::sync::atomic::AtomicUsize,
    saw_instructions: Arc<std::sync::atomic::AtomicBool>,
    advertised: Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait]
impl LlmClient<AgentSession, AgentReactContext> for SkillSearchLlm {
    async fn complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        session: &mut AgentSession,
        context: &mut AgentReactContext,
    ) -> Result<LlmResponse, LlmError> {
        let _ = &session;
        if self.step.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            *self.advertised.lock().unwrap() =
                context.tools.iter().map(|tool| tool.name.clone()).collect();
        }
        let step = self.step.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(match step {
            0 => make_tool_call_response("find_skill", "{\"query\":\"package instructions\"}"),
            1 => make_tool_call_response("load_skill", "{\"name\":\"extension-skill\"}"),
            _ => {
                let seen = context
                    .instructions
                    .iter()
                    .any(|instruction| instruction.instruction.contains("PACKAGE INSTRUCTIONS"));
                self.saw_instructions
                    .store(seen, std::sync::atomic::Ordering::SeqCst);
                make_text_response("Final Answer: followed the skill".to_string())
            }
        })
    }

    async fn stream_complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        _session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<react::llm::TokenStream, LlmError> {
        Ok(Box::pin(futures::stream::empty()))
    }

    fn supports_tools(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "skill-search-mock"
    }
}

/// A skill is found by what it does, then opened by name; the catalogue block
/// never had to be in the request.
#[tokio::test]
async fn a_skill_is_found_then_loaded_through_the_agent() {
    let advertised = Arc::new(std::sync::Mutex::new(Vec::new()));
    let saw = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut provider = LlmProvider::new();
    provider.register_vendor(
        "mock".to_string(),
        Box::new(SkillSearchLlm {
            step: std::sync::atomic::AtomicUsize::new(0),
            saw_instructions: saw.clone(),
            advertised: advertised.clone(),
        }),
    );
    let agent = Agent::new(AgentConfig::default(), Arc::new(provider));
    agent.add_live_skill(crate::skills::SkillContent {
        metadata: crate::skills::SkillMetadata::new(
            "extension-skill".to_string(),
            "Write the package instructions a release needs".to_string(),
            std::path::PathBuf::from("/ext/skills/extension-skill/SKILL.md"),
        ),
        instructions: "PACKAGE INSTRUCTIONS: do the thing".to_string(),
        skill_dir: std::path::PathBuf::from("/ext/skills/extension-skill"),
    });

    let answer = agent
        .react("package a release".to_string())
        .await
        .expect("the turn completes");

    assert!(answer.contains("followed the skill"), "{answer}");
    assert!(
        saw.load(std::sync::atomic::Ordering::SeqCst),
        "load_skill delivered the instructions the search named"
    );
    let seen = advertised.lock().unwrap().clone();
    assert!(
        seen.iter().any(|name| name == "find_skill"),
        "find_skill is advertised: {seen:?}"
    );
    assert!(
        seen.iter().any(|name| name == "load_skill"),
        "load_skill is advertised beside the search: {seen:?}"
    );
    assert_eq!(
        agent.active_skills(),
        vec!["extension-skill"],
        "the production load path activated the selected skill"
    );
    let events = agent.drain_skill_events();
    assert!(
        events.iter().any(|event| matches!(
            event,
            crate::skills::SkillEvent::Activated { skill, .. } if skill == "extension-skill"
        )),
        "the runtime recorded the activation: {events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            crate::skills::SkillEvent::Composed { skills, .. }
                if skills == &vec!["extension-skill".to_string()]
        )),
        "the runtime recorded the composed block: {events:?}"
    );
}

/// One MCP-shaped schema: several described parameters, the shape a server
/// ships. Real servers are wordier, so this understates the saving.
fn mcp_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "owner": {"type": "string", "description": "Repository owner"},
            "repo": {"type": "string", "description": "Repository name"},
            "state": {"type": "string", "enum": ["open", "closed", "all"], "description": "Issue state"},
            "labels": {"type": "array", "items": {"type": "string"}, "description": "Filter by labels"},
            "per_page": {"type": "integer", "minimum": 1, "maximum": 100, "description": "Results per page"},
            "page": {"type": "integer", "minimum": 1, "description": "Page number"}
        },
        "required": ["owner", "repo"]
    })
}

/// What the request carries for one tool, as the provider serialises it.
fn tool_chars(tool: &react::llm::LlmTool) -> usize {
    tool.name.len() + tool.description.len() + tool.parameters.to_string().len()
}

/// Withholding an MCP namespace must pay for the pair that replaces it.
///
/// The whole reason for deferral is prompt bytes: the advertised array is part
/// of every step's cached prefix. This pins the arithmetic in both directions -
/// what leaves, and what the discovery search plus its prompt paragraph
/// costs - so a future change that makes it expensive, or the filter
/// ineffective, fails here rather than in a bill.
#[test]
fn withholding_an_mcp_namespace_pays_for_the_discovery_pair() {
    let mut agent = Agent::new(AgentConfig::default(), Arc::new(make_llm_provider()));
    const SERVERS: usize = 40;
    for index in 0..SERVERS {
        let name = format!("github_tool_{index}");
        agent.add_tool(Arc::new(FunctionTool::new(
            &name,
            "Work with a GitHub issue in the connected repository",
            mcp_schema(),
            |_args| Ok(serde_json::json!({})),
        )));
        agent.mark_mcp_tool(&name);
    }

    let full: usize = agent.all_tools().iter().map(tool_chars).sum();
    let advertised: usize = agent.model_tools().iter().map(tool_chars).sum();
    let pair: usize = [crate::agent::agentic::discovery::find_tool_definition()]
        .iter()
        .map(tool_chars)
        .sum();
    let paragraph = crate::agent::prompts::DISCOVERY_GUIDANCE.len() + 2;

    let saved = full - advertised;
    println!(
        "mcp={SERVERS} full={full} advertised={advertised} pair={pair} paragraph={paragraph} saved={saved}"
    );

    // The search is INSIDE `model_tools`, so the count difference is what was
    // withheld minus the one entry that replaced it.
    assert_eq!(
        agent.all_tools().len() as i64 - agent.model_tools().len() as i64,
        SERVERS as i64 - 1,
        "every MCP tool leaves the array, and the search joins it"
    );
    let net = saved.saturating_sub(paragraph);
    assert!(
        net > 4 * pair,
        "deferring {SERVERS} MCP tools saves {net} chars after paying {paragraph} for the paragraph"
    );
}

/// The discovery paragraph is spent only where it is true: an agent that
/// withholds nothing never names `find_tool`.
#[tokio::test]
async fn the_discovery_paragraph_appears_only_when_a_tool_is_withheld() {
    let seen = Arc::new(std::sync::Mutex::new(None));
    let mut provider = LlmProvider::new();
    provider.register_vendor(
        "mock".to_string(),
        Box::new(PersonaCapturingLlm(seen.clone())),
    );
    let agent = Agent::new(AgentConfig::default(), Arc::new(provider));
    let _ = agent.react("task".to_string()).await;
    let plain = seen.lock().unwrap().clone().expect("a persona");
    assert!(
        !plain.contains("find_tool"),
        "nothing is withheld, so nothing points at find_tool"
    );

    let seen = Arc::new(std::sync::Mutex::new(None));
    let mut provider = LlmProvider::new();
    provider.register_vendor(
        "mock".to_string(),
        Box::new(PersonaCapturingLlm(seen.clone())),
    );
    let mut agent = Agent::new(AgentConfig::default(), Arc::new(provider));
    agent.add_tool(Arc::new(FunctionTool::new(
        "hidden",
        "withheld",
        serde_json::json!({"type": "object"}),
        |_args| Ok(serde_json::json!({})),
    )));
    let agent = agent.defer_tool("hidden");
    let _ = agent.react("task".to_string()).await;
    let guided = seen.lock().unwrap().clone().expect("a persona");
    assert!(
        guided.starts_with(crate::agent::prompts::HARNESS_IDENTITY),
        "identity opens the prompt"
    );
    assert_eq!(
        guided
            .matches(crate::agent::prompts::HARNESS_IDENTITY)
            .count(),
        1,
        "the identity line is not duplicated by the assembly"
    );
    assert!(
        guided.contains(crate::agent::prompts::DISCOVERY_GUIDANCE),
        "a withheld tool comes with the way back to it"
    );
    let priority = guided
        .find(crate::agent::prompts::DISCOVERY_PRIORITY)
        .expect("the priority heading");
    let contract = guided
        .find(crate::agent::prompts::WORKFLOW_GUIDANCE)
        .expect("the standing contract");
    let identity = guided
        .find(crate::agent::prompts::HARNESS_IDENTITY)
        .expect("the identity line");
    assert!(
        identity < priority && priority < contract,
        "identity opens, discovery precedes the standing contract"
    );
}

/// The skill paragraph rides the same priority block: a searched skill is
/// announced before the work starts, and the paragraph that only applies to a
/// withheld tool is left out.
#[tokio::test]
async fn the_skill_paragraph_opens_the_prompt_when_skills_are_searched() {
    let seen = Arc::new(std::sync::Mutex::new(None));
    let mut provider = LlmProvider::new();
    provider.register_vendor(
        "mock".to_string(),
        Box::new(PersonaCapturingLlm(seen.clone())),
    );
    let agent = Agent::new(AgentConfig::default(), Arc::new(provider));
    agent.add_live_skill(crate::skills::SkillContent {
        metadata: crate::skills::SkillMetadata::new(
            "review-skill".to_string(),
            "review a change".to_string(),
            std::path::PathBuf::from("/skills/review-skill/SKILL.md"),
        ),
        instructions: "review it".to_string(),
        skill_dir: std::path::PathBuf::from("/skills/review-skill"),
    });

    let _ = agent.react("task".to_string()).await;
    let guided = seen.lock().unwrap().clone().expect("a persona");
    assert!(
        guided.starts_with(crate::agent::prompts::HARNESS_IDENTITY),
        "identity opens the prompt"
    );
    assert!(
        guided.contains(crate::agent::prompts::DISCOVERY_PRIORITY),
        "a searched skill is announced before the work starts"
    );
    assert!(guided.contains(crate::agent::prompts::SKILL_DISCOVERY_GUIDANCE));
    assert!(
        !guided.contains(crate::agent::prompts::DISCOVERY_GUIDANCE),
        "nothing is withheld, so find_tool is not named"
    );
    assert!(
        guided.contains(crate::agent::prompts::WORKFLOW_GUIDANCE),
        "the standing contract still follows the priority block"
    );
}

/// A mock that asks for `load_skill` first and reports, on the second call,
/// whether the engine answered with the package's instructions.
struct LoaderLlm {
    calls: std::sync::atomic::AtomicUsize,
    saw_instructions: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait]
impl LlmClient<AgentSession, AgentReactContext> for LoaderLlm {
    async fn complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        session: &mut AgentSession,
        context: &mut AgentReactContext,
    ) -> Result<LlmResponse, LlmError> {
        if self
            .calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            == 0
        {
            return Ok(make_tool_call_response(
                "load_skill",
                r#"{"name":"extension-skill"}"#,
            ));
        }
        let _ = &session;
        let seen = context
            .instructions
            .iter()
            .any(|instruction| instruction.instruction.contains("PACKAGE INSTRUCTIONS"));
        self.saw_instructions
            .store(seen, std::sync::atomic::Ordering::Relaxed);
        Ok(make_text_response("final".to_string()))
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

/// A build whose ONLY skill comes from a package still gets `load_skill`.
///
/// The catalogue names the package's skill, so a build with no base skills used
/// to advertise one it could not open: the loader was gated on the pre-extension
/// set and never registered. A catalogue entry the loader cannot resolve is
/// worse than no entry - the model has been told the skill exists.
#[tokio::test]
async fn a_package_only_skill_is_loadable_by_the_model() {
    let saw_instructions = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let provider = {
        let mut provider = LlmProvider::new();
        provider.register_vendor(
            "toolmock".to_string(),
            Box::new(LoaderLlm {
                calls: std::sync::atomic::AtomicUsize::new(0),
                saw_instructions: Arc::clone(&saw_instructions),
            }),
        );
        provider
    };
    let agent = Agent::new(AgentConfig::default(), Arc::new(provider));
    agent.add_live_skill(crate::skills::SkillContent {
        metadata: crate::skills::SkillMetadata::new(
            "extension-skill".to_string(),
            "a skill from a package".to_string(),
            std::path::PathBuf::from("/ext/skills/extension-skill/SKILL.md"),
        ),
        instructions: "PACKAGE INSTRUCTIONS".to_string(),
        skill_dir: std::path::PathBuf::from("/ext/skills/extension-skill"),
    });

    assert!(
        agent.skill("extension-skill").is_some(),
        "the reader's /name gesture resolves the same set"
    );

    let _ = agent.react("do the thing".to_string()).await;

    assert!(
        saw_instructions.load(std::sync::atomic::Ordering::Relaxed),
        "the model's load_skill call must return the package's instructions"
    );
}

/// The skill catalogue in the system prompt is the model's only way to learn a
/// skill exists; a package's skills live in the live overlay too, so the
/// catalogue must be drawn from both sets.
#[test]
fn prepare_context_advertises_live_extension_skills() {
    let agent = Agent::new(AgentConfig::default(), Arc::new(make_llm_provider()))
        .with_skill_catalogue(true);
    agent.add_live_skill(crate::skills::SkillContent {
        metadata: crate::skills::SkillMetadata::new(
            "extension-skill".to_string(),
            "a skill from a package".to_string(),
            std::path::PathBuf::from("/ext/skills/extension-skill/SKILL.md"),
        ),
        instructions: "do the thing".to_string(),
        skill_dir: std::path::PathBuf::from("/ext/skills/extension-skill"),
    });

    let context = agent.prepare_context();
    assert!(
        context
            .skills
            .iter()
            .any(|skill| skill.name == "extension-skill"),
        "catalogue mode carries the skill"
    );
    assert!(
        !context.tools.iter().any(|tool| tool.name == "find_skill"),
        "the catalogue and the search are alternatives, not layers"
    );
}

/// The default mode searches: the request carries no skill metadata, and the
/// one tool that reaches a skill is advertised in its place.
#[test]
fn a_live_extension_skill_is_searched_not_catalogued() {
    let agent = Agent::new(AgentConfig::default(), Arc::new(make_llm_provider()));
    agent.add_live_skill(crate::skills::SkillContent {
        metadata: crate::skills::SkillMetadata::new(
            "extension-skill".to_string(),
            "a skill from a package".to_string(),
            std::path::PathBuf::from("/ext/skills/extension-skill/SKILL.md"),
        ),
        instructions: "do the thing".to_string(),
        skill_dir: std::path::PathBuf::from("/ext/skills/extension-skill"),
    });

    let context = agent.prepare_context();
    assert!(
        context.skills.is_empty(),
        "no catalogue block by default: {:?}",
        context.skills
    );
    assert!(
        context.tools.iter().any(|tool| tool.name == "find_skill"),
        "find_skill replaces the catalogue: {:?}",
        context
            .tools
            .iter()
            .map(|tool| &tool.name)
            .collect::<Vec<_>>()
    );
}

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

/// The model's catalogue and its loader answer from ONE rule: a skill whose
/// manifest reserves it for the reader (disable-model-invocation: true) stays
/// registered - the reader's slash-name gesture loads it - but the request the
/// model receives never names it.
#[test]
fn a_user_only_skill_is_not_in_the_models_catalogue() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("bos-skill-policy-{nanos}"));
    let write = |name: &str, restriction: &str| {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).expect("the skill directory");
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: a test skill\n{restriction}---\n\nbody\n"),
        )
        .expect("the manifest");
    };
    write("open", "");
    write("reserved", "disable-model-invocation: true\n");

    let mut agent = Agent::new(AgentConfig::default(), Arc::new(make_llm_provider()))
        .with_skill_catalogue(true);
    agent
        .register_skills_from_dir(root.clone())
        .expect("the skills register");

    // The reader's path keeps both: the gesture resolves against the agent, and
    // a skill reserved from the model is exactly the one a reader must name.
    assert!(
        agent.skill("reserved").is_some(),
        "registered for the reader"
    );
    assert!(agent.skill("open").is_some());

    // The model's catalogue carries only the open one.
    let context = agent.prepare_context();
    let names: Vec<&str> = context.skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["open"],
        "only the model-invocable skill is advertised: {names:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A vendor that asks for load_skill once, then reports what the engine put in
/// the tool result.
struct SkillLoadingLlm {
    name: String,
    seen: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl LlmClient<AgentSession, AgentReactContext> for SkillLoadingLlm {
    async fn complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        session: &mut AgentSession,
        _context: &mut AgentReactContext,
    ) -> Result<LlmResponse, LlmError> {
        let call = self
            .calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if call == 0 {
            return Ok(make_tool_call_response(
                "load_skill",
                &format!("{{\"name\":\"{}\"}}", self.name),
            ));
        }
        let result = session
            .messages()
            .iter()
            .rev()
            .find_map(|message| match message {
                react::llm::LlmMessage::ToolResult { content, .. } => Some(content.clone()),
                _ => None,
            });
        *self.seen.lock().unwrap() = result;
        Ok(make_text_response("final".to_string()))
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
        "skill-loading-mock"
    }
}

/// The model's load_skill answers from the SAME policy the catalogue uses: a
/// skill reserved for the reader is refused with the reference's wording, and
/// its instructions never reach the model; an ordinary skill still loads.
#[tokio::test]
async fn the_loader_refuses_a_skill_reserved_from_the_model() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("bos-skill-loader-{nanos}"));
    let write = |name: &str, restriction: &str, body: &str| {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).expect("the skill directory");
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: a test skill\n{restriction}---\n\n{body}\n"),
        )
        .expect("the manifest");
    };
    write("open", "", "OPEN BODY");
    write(
        "reserved",
        "disable-model-invocation: true\n",
        "RESERVED BODY",
    );

    let load = |name: &str| {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
        let mut provider = LlmProvider::new();
        provider.register_vendor(
            "skill-loading-mock".to_string(),
            Box::new(SkillLoadingLlm {
                name: name.to_string(),
                seen: seen.clone(),
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
        );
        (provider, seen)
    };

    let (provider, refused) = load("reserved");
    let mut agent = Agent::new(AgentConfig::default(), Arc::new(provider));
    agent
        .register_skills_from_dir(root.clone())
        .expect("the skills register");
    let _ = agent.react("load the reserved one".to_string()).await;
    let refused = refused
        .lock()
        .unwrap()
        .clone()
        .expect("the loader answered");
    assert!(
        refused.contains("not available for model invocation"),
        "the refusal is the reference's: {refused}"
    );
    assert!(
        !refused.contains("RESERVED BODY"),
        "the reserved body must not travel: {refused}"
    );

    let (provider, loaded) = load("open");
    let mut agent = Agent::new(AgentConfig::default(), Arc::new(provider));
    agent
        .register_skills_from_dir(root.clone())
        .expect("the skills register");
    let _ = agent.react("load the open one".to_string()).await;
    let loaded = loaded.lock().unwrap().clone().expect("the loader answered");
    assert!(
        loaded.contains("\"instructions_in_context\":true")
            && loaded.contains("\"skills\":[\"open\"]"),
        "an ordinary skill still activates and hands its block to the context: {loaded}"
    );
    let _ = std::fs::remove_dir_all(&root);
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

/// A custom provider prefix keeps the OpenAI-compatible transport, and the
/// configured Anthropic protocol reaches the wire as Messages: the gateway
/// scenario the Models pane's protocol picker configures.
#[tokio::test]
async fn a_custom_prefix_honors_the_anthropic_protocol_on_the_wire() {
    use react::llm::{ApiMode, Content, LlmRequest};
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[derive(Default)]
    struct Captured {
        path: String,
        x_api_key: String,
    }
    let captured = Arc::new(Mutex::new(Vec::<Captured>::new()));
    let sink = Arc::clone(&captured);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let sink = Arc::clone(&sink);
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                let head_end = loop {
                    let Ok(n) = socket.read(&mut tmp).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break pos + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
                let mut lines = head.lines();
                let path = lines
                    .next()
                    .unwrap_or_default()
                    .split(' ')
                    .nth(1)
                    .unwrap_or_default()
                    .to_string();
                let x_api_key = lines
                    .find_map(|line| {
                        let (n, v) = line.split_once(':')?;
                        n.trim()
                            .eq_ignore_ascii_case("x-api-key")
                            .then(|| v.trim().to_string())
                    })
                    .unwrap_or_default();
                sink.lock()
                    .expect("lock")
                    .push(Captured { path, x_api_key });
                let sse = concat!(
                    "event: message_start\n",
                    "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1}}}\n\n",
                    "event: content_block_delta\n",
                    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"gateway says hi\"}}\n\n",
                    "event: message_stop\n",
                    "data: {\"type\":\"message_stop\"}\n\n",
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    sse.len(),
                    sse
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });

    let config = AgentConfig {
        name: "gateway-agent".to_string(),
        model: "gateway/m".to_string(),
        base_url: format!("http://{addr}"),
        api_key: "test-key".to_string(),
        api_mode: "anthropic".to_string(),
        ..Default::default()
    };
    let (name, vendor) = super::llm::build_vendor(&config);
    assert_eq!(name, "gateway", "the custom prefix is the router key");
    let outcome = vendor
        .complete(
            None,
            LlmRequest {
                model: "m".to_string(),
                input: Content::text("hi"),
                api_mode: ApiMode::Anthropic,
                ..Default::default()
            },
            &mut crate::agent::context::AgentSession::new(),
            &mut crate::agent::context::AgentReactContext::default(),
        )
        .await
        .expect("a custom gateway speaks the configured protocol");
    assert_eq!(outcome.text(), Some("gateway says hi"));
    let seen = captured.lock().expect("lock");
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].path, "/v1/messages");
    assert_eq!(seen[0].x_api_key, "test-key");
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
        session.add_user(format!("old one {}", "x".repeat(1200)));
        session.add_assistant(format!("reply one {}", "y".repeat(1200)));
        session.add_user(format!("old two {}", "x".repeat(1200)));
        session.add_assistant(format!("reply two {}", "y".repeat(1200)));
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
        session.add_user(format!("old one {}", "x".repeat(1200)));
        session.add_assistant(format!("reply one {}", "y".repeat(1200)));
        session.add_user(format!("old two {}", "x".repeat(1200)));
        session.add_assistant(format!("reply two {}", "y".repeat(1200)));
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

/// A foldable span under the floor folds NOTHING and pays no summarizer call.
///
/// The automatic trigger prices the whole request while only messages fold, so
/// a shell with a large fixed overhead used to reach here every turn with a
/// span of a few hundred tokens - each visit a full completion that a verbose
/// checkpoint then refused as NotSmaller. The floor returns the honest
/// "nothing worth folding" before any request is made.
#[tokio::test]
async fn compact_now_skips_a_span_that_cannot_pay_for_its_own_call() {
    let (agent, seen) = agent_with_script(Ok("checkpoint".to_string()), 0);
    {
        let mut session = agent.session();
        session.add_user("old one".to_string());
        session.add_assistant("reply one".to_string());
        session.add_user("old two".to_string());
    }
    let signal = std::sync::atomic::AtomicBool::new(false);
    let outcome = agent
        .compact_now(0, &signal)
        .await
        .expect("the compaction request");
    assert!(outcome.is_none(), "a tiny span folds nothing: {outcome:?}");
    assert!(
        seen.lock().unwrap().is_empty(),
        "no summarizer request was paid for the tiny span"
    );
    assert_eq!(
        agent.session().messages().len(),
        3,
        "the history is untouched when the span is under the floor"
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
    // Long enough to clear the compact_now span floor: these tests pin the
    // summarizer contract, which only runs once a span is worth a call.
    session.add_user(format!("old one {}", "x".repeat(1200)));
    session.add_assistant(format!("reply one {}", "y".repeat(1200)));
    session.add_user(format!("old two {}", "x".repeat(1200)));
    session.add_assistant(format!("reply two {}", "y".repeat(1200)));
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

#[test]
fn extension_prompt_sections_revoke_only_their_exact_delimited_section() {
    let mut agent = Agent::from_config(AgentConfig {
        system_prompt: "base persona".to_string(),
        ..AgentConfig::default()
    });
    agent.append_system_prompt("extension first");
    agent.append_system_prompt("extension last");

    assert!(agent.remove_system_prompt_section("extension first"));
    assert_eq!(
        agent.config().system_prompt,
        "base persona\n\nextension last"
    );
    assert!(agent.remove_system_prompt_section("extension last"));
    assert_eq!(agent.config().system_prompt, "base persona");
}

#[test]
fn extension_prompt_section_removal_handles_first_and_only_sections() {
    let mut agent = Agent::from_config(AgentConfig {
        system_prompt: String::new(),
        ..AgentConfig::default()
    });
    agent.append_system_prompt("only extension");
    assert!(agent.remove_system_prompt_section("only extension"));
    assert!(agent.config().system_prompt.is_empty());

    agent.append_system_prompt("first extension");
    agent.append_system_prompt("base follow-up");
    assert!(agent.remove_system_prompt_section("first extension"));
    assert_eq!(agent.config().system_prompt, "base follow-up");
}

#[test]
fn extension_prompt_section_removal_rejects_substring_matches() {
    let mut agent = Agent::from_config(AgentConfig {
        system_prompt: "base persona".to_string(),
        ..AgentConfig::default()
    });
    agent.append_system_prompt("extension section with suffix");

    assert!(!agent.remove_system_prompt_section("extension section"));
    assert_eq!(
        agent.config().system_prompt,
        "base persona\n\nextension section with suffix"
    );
}
