//! End-to-end tests for `Agent::react` against a local mock Responses API
//! server: a full tool-call roundtrip and per-model config default resolution
//! flowing all the way through to the wire format.

#[path = "../../react/tests/common/mock_llm_server.rs"]
mod mock_llm_server;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use agent::agent::agentic::{Agent, AgentConfig, LlmProvider};
use agent::tools::FunctionTool;
use mock_llm_server::{CapturedRequest, MockLlmServer, MockReply};
use react::llm::vendor::OpenAiVendor;
use serde_json::json;

fn responses_response(text: &str) -> serde_json::Value {
    json!({
        "id": "resp-mock",
        "object": "response",
        "created_at": 1720000000,
        "status": "completed",
        "model": "gpt-4",
        "output": [{
            "type": "message",
            "id": "msg_1",
            "role": "assistant",
            "content": [{"type": "output_text", "text": text, "annotations": []}]
        }],
        "usage": {"input_tokens": 10, "output_tokens": 5, "total_tokens": 15}
    })
}

#[tokio::test]
async fn agent_react_responses_tool_roundtrip() {
    // Turn 1: the model requests the `add` tool. Turn 2: it answers.
    let call = Arc::new(AtomicUsize::new(0));
    let server = {
        let call = call.clone();
        MockLlmServer::start(move |req: &CapturedRequest| {
            assert_eq!(req.path, "/responses");
            let n = call.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                assert_eq!(req.body["reasoning"]["effort"], json!("high"));
                let tools = req.body["tools"].as_array().expect("tools on turn 1");
                assert!(tools.iter().any(|t| t["name"] == json!("add")));
                MockReply::Json(json!({
                    "id": "resp-tool",
                    "object": "response",
                    "created_at": 1,
                    "status": "completed",
                    "model": "gpt-4",
                    "output": [{
                        "type": "function_call",
                        "id": "fc_1",
                        "call_id": "call_add_1",
                        "name": "add",
                        "arguments": "{\"a\": 1, \"b\": 2}"
                    }],
                    "usage": {"input_tokens": 5, "output_tokens": 2, "total_tokens": 7}
                }))
            } else {
                // The tool result must be fed back as a function_call_output item.
                let input = req.body["input"].as_array().expect("input on turn 2");
                let fco = input
                    .iter()
                    .find(|item| item["type"] == json!("function_call_output"))
                    .expect("function_call_output item on turn 2");
                assert_eq!(fco["call_id"], json!("call_add_1"));
                assert!(
                    fco["output"].as_str().unwrap_or_default().contains("sum"),
                    "tool result output: {fco:?}"
                );
                MockReply::Json(responses_response("3"))
            }
        })
    }
    .await;

    let mut llm = LlmProvider::new();
    llm.register_vendor(
        "openai".to_string(),
        Box::new(OpenAiVendor::new(
            server.url(),
            "gpt-4".into(),
            "sk-mock".into(),
        )),
    );

    let mut agent = Agent::new(
        AgentConfig {
            model: "gpt-4".to_string(),
            api_mode: "responses".to_string(),
            reasoning_effort: Some("high".to_string()),
            ..Default::default()
        },
        Arc::new(llm),
    );

    agent.add_tool(Arc::new(FunctionTool::new(
        "add",
        "Add two integers",
        json!({
            "type": "object",
            "properties": {
                "a": {"type": "integer"},
                "b": {"type": "integer"}
            },
            "required": ["a", "b"]
        }),
        |args| -> Result<serde_json::Value, react::tool::ToolError> {
            let a = args["a"].as_i64().unwrap_or(0);
            let b = args["b"].as_i64().unwrap_or(0);
            Ok(json!({"sum": a + b}))
        },
    )));

    let answer = agent
        .react("What is 1 + 2?")
        .await
        .expect("react() succeeds");
    assert_eq!(answer, "3");
    assert_eq!(call.load(Ordering::SeqCst), 2);
    server.shutdown().await;
}

#[tokio::test]
async fn agent_per_model_defaults_reach_wire() {
    // A home config section `[llm.prod]` matching the agent's model must drive
    // api_mode + reasoning_effort all the way to the /responses request.
    let home = json!({
        "llm": {
            "prod": {
                "model": "openai/gpt-4",
                "api_mode": "responses",
                "reasoning_effort": "high"
            }
        }
    });

    let server =
        MockLlmServer::start(|_req: &CapturedRequest| MockReply::Json(responses_response("ok")))
            .await;

    let mut config = AgentConfig {
        model: "openai/gpt-4".to_string(),
        ..Default::default()
    };
    agent::agent::config::apply_model_defaults_from(&mut config, &home);
    assert_eq!(config.api_mode, "responses");
    assert_eq!(config.reasoning_effort.as_deref(), Some("high"));

    let mut llm = LlmProvider::new();
    llm.register_vendor(
        "openai".to_string(),
        Box::new(OpenAiVendor::new(
            server.url(),
            "gpt-4".into(),
            "sk-mock".into(),
        )),
    );
    let agent = Agent::new(config, Arc::new(llm));

    let answer = agent.react("hello").await.expect("react() succeeds");
    assert_eq!(answer, "ok");

    let reqs = server.requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].path, "/responses");
    assert_eq!(reqs[0].body["reasoning"]["effort"], json!("high"));
    assert_eq!(reqs[0].body["model"], json!("gpt-4"));
    server.shutdown().await;
}
