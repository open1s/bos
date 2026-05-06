use async_trait::async_trait;
use std::sync::{Arc, Mutex};

use react::engine::ReActEngineBuilder;
use react::llm::vendor::ChatCompletionResponse;
use react::llm::{LlmClient, LlmContext, LlmError, LlmRequest, LlmResponse, LlmResponseResult, LlmSession, TokenStream, Skill, ReactContext};
use react::runtime::ReActApp;

#[derive(Default)]
struct TestApp;
impl ReActApp for TestApp {
    type Session = LlmSession;
    type Context = LlmContext;
}

fn make_text_response(content: String) -> LlmResponseResult {
    Ok(LlmResponse::OpenAI(ChatCompletionResponse {
        id: "test-123".to_string(),
        object: "chat.completion".to_string(),
        created: 1234567890,
        model: "test-model".to_string(),
        choices: vec![react::llm::vendor::Choice {
            index: 0,
            message: react::llm::vendor::ChatMessage {
                role: "assistant".to_string(),
                content: Some(content),
                tool_calls: None,
                function_call: None,
                reasoning_content: None,
                extra: serde_json::Value::Object(serde_json::Map::new()),
            },
            stop_reason: None,
            finish_reason: Some("stop".to_string()),
            logprobs: None,
        }],
        usage: None,
        system_fingerprint: None,
        nvext: None,
    }))
}

fn build_tools_from_context(context: &impl ReactContext) -> Vec<serde_json::Value> {
    context
        .tools()
        .map(|tools| {
            tools
                .into_iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters
                        }
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn test_load_skill_added_to_tools_when_skills_exist() {
    struct ToolCapturingLlm {
        captured_tools: Arc<Mutex<Option<Vec<serde_json::Value>>>>,
    }

    #[async_trait]
    impl LlmClient<LlmSession, LlmContext> for ToolCapturingLlm {
        async fn complete(
            &self,
            _request: LlmRequest,
            _session: &mut LlmSession,
            context: &mut LlmContext,
        ) -> LlmResponseResult {
            let tools = build_tools_from_context(context);
            let mut captured = self.captured_tools.lock().unwrap();
            *captured = Some(tools);
            make_text_response("Final Answer: test".to_string())
        }

        async fn stream_complete(
            &self,
            _request: LlmRequest,
            _session: &mut LlmSession,
            _context: &mut LlmContext,
        ) -> Result<TokenStream, LlmError> {
            Ok(Box::pin(futures::stream::empty()))
        }

        fn supports_tools(&self) -> bool {
            true
        }
        fn provider_name(&self) -> &'static str {
            "tool_capturing"
        }
    }

    let captured_tools = Arc::new(Mutex::new(None));
    let mock_llm = ToolCapturingLlm {
        captured_tools: captured_tools.clone(),
    };

    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(mock_llm))
        .max_steps(1)
        .build()
        .expect("Failed to build engine");

    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut session = LlmSession::default();
    let mut context = LlmContext::default();

    context.skills.push(Skill {
        category: "test".to_string(),
        description: "A test skill".to_string(),
        name: "test skill".to_string(),
    });

    rt.block_on(async {
        engine.react(LlmRequest::new("test"), &mut session, &mut context).await
    })
    .unwrap();

    let tools = captured_tools.lock().unwrap();
    let tools = tools.as_ref().expect("No tools captured");

    println!("Tools built from context: {:?}", tools);

    let has_load_skill = tools.iter().any(|t| {
        t.get("function")
            .and_then(|f| f.get("name"))
            .and_then(|n| n.as_str())
            .map(|s| s == "load_skill")
            .unwrap_or(false)
    });

    assert!(has_load_skill, "load_skill tool should be included when skills exist, but tools were: {:?}", tools);
}

#[test]
fn test_no_load_skill_when_no_skills() {
    struct ToolCapturingLlm {
        captured_tools: Arc<Mutex<Option<Vec<serde_json::Value>>>>,
    }

    #[async_trait]
    impl LlmClient<LlmSession, LlmContext> for ToolCapturingLlm {
        async fn complete(
            &self,
            _request: LlmRequest,
            _session: &mut LlmSession,
            context: &mut LlmContext,
        ) -> LlmResponseResult {
            let tools = build_tools_from_context(context);
            let mut captured = self.captured_tools.lock().unwrap();
            *captured = Some(tools);
            make_text_response("Final Answer: test".to_string())
        }

        async fn stream_complete(
            &self,
            _request: LlmRequest,
            _session: &mut LlmSession,
            _context: &mut LlmContext,
        ) -> Result<TokenStream, LlmError> {
            Ok(Box::pin(futures::stream::empty()))
        }

        fn supports_tools(&self) -> bool {
            true
        }
        fn provider_name(&self) -> &'static str {
            "tool_capturing"
        }
    }

    let captured_tools = Arc::new(Mutex::new(None));
    let mock_llm = ToolCapturingLlm {
        captured_tools: captured_tools.clone(),
    };

    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(mock_llm))
        .max_steps(1)
        .build()
        .expect("Failed to build engine");

    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut session = LlmSession::default();
    let context = LlmContext::default();

    rt.block_on(async {
        engine.react(LlmRequest::new("test"), &mut session, &mut context.clone()).await
    })
    .unwrap();

    let tools = captured_tools.lock().unwrap();
    let tools = tools.as_ref().expect("No tools captured");

    let _has_load_skill = tools.iter().any(|t| {
        t.get("function")
            .and_then(|f| f.get("name"))
            .and_then(|n| n.as_str())
            .map(|s| s == "load_skill")
            .unwrap_or(false)
    });
}