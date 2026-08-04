//! End-to-end wire-format tests for the OpenAI vendor against a local mock
//! HTTP server. Verifies that `reasoning_effort` and `api_mode` produce the
//! correct request bodies and paths for both the Chat Completions and
//! Responses protocols, and that hosted tools serialize correctly.

mod common;

use serde_json::{json, Value};

use common::mock_llm_server::{CapturedRequest, MockLlmServer, MockReply};
use react::llm::vendor::OpenAiVendor;
use react::llm::{
    ApiMode, LlmClient, LlmContext, LlmMessage, LlmRequest, LlmResponse, LlmSession, LlmTool,
    ReactContext, ReasoningEffort, StreamToken,
};

fn chat_completion(text: &str) -> Value {
    json!({
        "id": "chatcmpl-mock",
        "object": "chat.completion",
        "created": 1720000000,
        "model": "gpt-4",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": text},
            "finish_reason": "stop",
            "logprobs": null
        }],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
        "system_fingerprint": null
    })
}

fn chat_chunk(text: &str) -> Value {
    json!({
        "id": "chatcmpl-mock",
        "object": "chat.completion.chunk",
        "created": 1720000000,
        "model": "gpt-4",
        "choices": [{
            "index": 0,
            "delta": {"content": text},
            "finish_reason": null,
            "logprobs": null
        }]
    })
}

fn responses_response(text: &str) -> Value {
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

fn responses_completed_event() -> Value {
    json!({
        "type": "response.completed",
        "response": {
            "id": "resp-mock",
            "object": "response",
            "created_at": 1720000000,
            "status": "completed",
            "model": "gpt-4",
            "output": [{
                "type": "message",
                "id": "msg_1",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "Hello world", "annotations": []}]
            }],
            "usage": {"input_tokens": 10, "output_tokens": 5, "total_tokens": 15}
        }
    })
}

async fn collect_tokens(stream: react::llm::TokenStream) -> Vec<StreamToken> {
    use futures::StreamExt;
    stream
        .collect::<Vec<Result<StreamToken, react::llm::LlmError>>>()
        .await
        .into_iter()
        .map(|r| r.expect("stream token"))
        .collect()
}

/// Asserts the tokens are exactly `texts` followed by a trailing `Done`.
/// When `expect_usage` is true, a `Usage` token sits between the texts and
/// `Done` (the Responses protocol emits usage before completion).
fn assert_text_then_done(tokens: &[StreamToken], texts: &[&str], expect_usage: bool) {
    let usage_slot = usize::from(expect_usage);
    assert_eq!(
        tokens.len(),
        texts.len() + 1 + usage_slot,
        "tokens: {tokens:?}"
    );
    for (i, text) in texts.iter().enumerate() {
        assert!(
            matches!(&tokens[i], StreamToken::Text(t) if t == text),
            "tokens: {tokens:?}"
        );
    }
    if expect_usage {
        assert!(
            matches!(&tokens[texts.len()], StreamToken::Usage(_)),
            "tokens: {tokens:?}"
        );
    }
    assert!(
        matches!(tokens.last(), Some(StreamToken::Done)),
        "expected trailing Done, got {tokens:?}"
    );
}

#[tokio::test]
async fn chat_complete_reasoning_effort_and_path() {
    let server =
        MockLlmServer::start(|_req: &CapturedRequest| MockReply::Json(chat_completion("hi"))).await;
    let vendor = OpenAiVendor::new(server.url(), "gpt-4".into(), "sk-mock".into());

    let mut session = LlmSession::new();
    session.push(LlmMessage::user("hello"));
    let mut context = LlmContext::default();
    let req = LlmRequest::new("gpt-4").reasoning_effort(ReasoningEffort::High);

    let resp = vendor
        .complete(None, req, &mut session, &mut context)
        .await
        .expect("chat complete");

    let reqs = server.requests();
    assert_eq!(reqs.len(), 1);
    let first = &reqs[0];
    assert_eq!(first.path, "/chat/completions");
    assert_eq!(first.body["reasoning_effort"], json!("high"));
    assert_eq!(first.body["model"], json!("gpt-4"));
    assert_eq!(first.body["stream"], json!(false));
    assert!(matches!(resp, LlmResponse::OpenAI(_)));
    server.shutdown().await;
}

#[tokio::test]
async fn chat_complete_model_fallback_to_vendor_model() {
    let server =
        MockLlmServer::start(|_req: &CapturedRequest| MockReply::Json(chat_completion("hi"))).await;
    let vendor = OpenAiVendor::new(server.url(), "vendor-model".into(), "sk-mock".into());

    let mut session = LlmSession::new();
    session.push(LlmMessage::user("hello"));
    let mut context = LlmContext::default();
    let req = LlmRequest::new("");

    vendor
        .complete(None, req, &mut session, &mut context)
        .await
        .expect("chat complete");

    let reqs = server.requests();
    assert_eq!(reqs[0].body["model"], json!("vendor-model"));
    server.shutdown().await;
}

#[tokio::test]
async fn chat_stream_reasoning_effort_and_done_signal() {
    let server = MockLlmServer::start(|_req: &CapturedRequest| MockReply::Sse {
        events: vec![chat_chunk("Hello "), chat_chunk("world")],
        append_done: true,
    })
    .await;
    let vendor = OpenAiVendor::new(server.url(), "gpt-4".into(), "sk-mock".into());

    let mut session = LlmSession::new();
    session.push(LlmMessage::user("hello"));
    let mut context = LlmContext::default();
    let req = LlmRequest::new("gpt-4").reasoning_effort(ReasoningEffort::Medium);

    let stream = vendor
        .stream_complete(None, req, &mut session, &mut context)
        .await
        .expect("chat stream");

    let tokens = collect_tokens(stream).await;
    assert_text_then_done(&tokens, &["Hello ", "world"], false);

    let reqs = server.requests();
    assert_eq!(reqs[0].path, "/chat/completions");
    assert_eq!(reqs[0].body["reasoning_effort"], json!("medium"));
    assert_eq!(reqs[0].body["stream"], json!(true));
    server.shutdown().await;
}

#[tokio::test]
async fn responses_complete_reasoning_effort_nested() {
    let server =
        MockLlmServer::start(|_req: &CapturedRequest| MockReply::Json(responses_response("hi")))
            .await;
    let vendor = OpenAiVendor::new(server.url(), "gpt-4".into(), "sk-mock".into());

    let mut session = LlmSession::new();
    session.push(LlmMessage::system("Be concise."));
    session.push(LlmMessage::user("hello"));
    let mut context = LlmContext::default();
    let req = LlmRequest::new("gpt-4")
        .api_mode(ApiMode::Responses)
        .reasoning_effort(ReasoningEffort::High);

    let resp = vendor
        .complete(None, req, &mut session, &mut context)
        .await
        .expect("responses complete");

    let reqs = server.requests();
    assert_eq!(reqs.len(), 1);
    let first = &reqs[0];
    assert_eq!(first.path, "/responses");
    assert_eq!(first.body["reasoning"]["effort"], json!("high"));
    assert_eq!(first.body["model"], json!("gpt-4"));
    // System messages fold into `instructions`, not the input.
    assert_eq!(first.body["instructions"], json!("Be concise.\n"));
    // Input items carry the user message.
    let input = first.body["input"].as_array().expect("input array");
    assert!(input.iter().any(|item| item["role"] == json!("user")));
    assert!(matches!(resp, LlmResponse::Responses(_)));
    server.shutdown().await;
}

#[tokio::test]
async fn responses_stream_reasoning_effort_nested() {
    let server = MockLlmServer::start(|_req: &CapturedRequest| MockReply::Sse {
        events: vec![
            json!({
                "type": "response.output_text.delta",
                "item_id": "msg_1",
                "output_index": 0,
                "content_index": 0,
                "delta": "Hello "
            }),
            json!({
                "type": "response.output_text.delta",
                "item_id": "msg_1",
                "output_index": 0,
                "content_index": 0,
                "delta": "world"
            }),
            responses_completed_event(),
        ],
        append_done: false,
    })
    .await;
    let vendor = OpenAiVendor::new(server.url(), "gpt-4".into(), "sk-mock".into());

    let mut session = LlmSession::new();
    session.push(LlmMessage::user("hello"));
    let mut context = LlmContext::default();
    let req = LlmRequest::new("gpt-4")
        .api_mode(ApiMode::Responses)
        .reasoning_effort(ReasoningEffort::Medium);

    let stream = vendor
        .stream_complete(None, req, &mut session, &mut context)
        .await
        .expect("responses stream");

    let tokens = collect_tokens(stream).await;
    assert_text_then_done(&tokens, &["Hello ", "world"], true);

    let reqs = server.requests();
    assert_eq!(reqs[0].path, "/responses");
    assert_eq!(reqs[0].body["reasoning"]["effort"], json!("medium"));
    assert_eq!(reqs[0].body["stream"], json!(true));
    server.shutdown().await;
}

#[tokio::test]
async fn responses_stream_function_call_accumulation() {
    let server = MockLlmServer::start(|_req: &CapturedRequest| MockReply::Sse {
        events: vec![
            json!({
                "type": "response.output_item.added",
                "output_index": 0,
                "item": {"id": "fc_1", "type": "function_call", "call_id": "call_1",
                         "name": "add", "arguments": ""}
            }),
            json!({
                "type": "response.function_call_arguments.delta",
                "item_id": "fc_1", "output_index": 0,
                "delta": "{\"a\": 1,"
            }),
            json!({
                "type": "response.function_call_arguments.delta",
                "item_id": "fc_1", "output_index": 0,
                "delta": " \"b\": 2}"
            }),
            json!({
                "type": "response.function_call_arguments.done",
                "item_id": "fc_1", "output_index": 0,
                "arguments": "{\"a\": 1, \"b\": 2}"
            }),
            responses_completed_event(),
        ],
        append_done: false,
    })
    .await;
    let vendor = OpenAiVendor::new(server.url(), "gpt-4".into(), "sk-mock".into());

    let mut session = LlmSession::new();
    session.push(LlmMessage::user("1 + 2?"));
    let mut context = LlmContext::default();
    context.add_tool(LlmTool::function(
        "add",
        "Add two numbers",
        json!({
            "type": "object",
            "properties": {
                "a": {"type": "number"},
                "b": {"type": "number"}
            },
            "required": ["a", "b"]
        }),
    ));
    let req = LlmRequest::new("gpt-4").api_mode(ApiMode::Responses);

    let stream = vendor
        .stream_complete(None, req, &mut session, &mut context)
        .await
        .expect("responses stream");

    let tokens = collect_tokens(stream).await;
    let tool_calls: Vec<&StreamToken> = tokens
        .iter()
        .filter(|t| matches!(t, StreamToken::ToolCall { .. }))
        .collect();
    assert_eq!(
        tool_calls.len(),
        1,
        "expected one tool call, got {tokens:?}"
    );
    match tool_calls[0] {
        StreamToken::ToolCall { name, args, id } => {
            assert_eq!(name, "add");
            assert_eq!(*args, json!({"a": 1, "b": 2}));
            assert_eq!(id.as_deref(), Some("call_1"));
        }
        _ => unreachable!(),
    }

    // The tools array must be sent on the wire for the responses API.
    let reqs = server.requests();
    let tools = reqs[0].body["tools"].as_array().expect("tools array");
    assert!(tools.iter().any(|t| t["name"] == json!("add")));
    server.shutdown().await;
}

#[tokio::test]
async fn hosted_tools_wire_format_responses() {
    let server =
        MockLlmServer::start(|_req: &CapturedRequest| MockReply::Json(responses_response("hi")))
            .await;
    let vendor = OpenAiVendor::new(server.url(), "gpt-4".into(), "sk-mock".into());

    let mut session = LlmSession::new();
    session.push(LlmMessage::user("hello"));
    let mut context = LlmContext::default();
    context.add_tool(LlmTool::web_search(Some(
        json!({"search_context_size": "medium"}),
    )));
    context.add_tool(LlmTool::file_search(Some(json!({"max_num_results": 5}))));
    context.add_tool(LlmTool::computer_use(Some(json!({"display_width": 1024}))));
    let req = LlmRequest::new("gpt-4").api_mode(ApiMode::Responses);

    vendor
        .complete(None, req, &mut session, &mut context)
        .await
        .expect("responses complete");

    let reqs = server.requests();
    let tools = reqs[0].body["tools"].as_array().expect("tools array");
    let by_type = |kind: &str| tools.iter().find(|t| t["type"] == json!(kind));

    let web = by_type("web_search").expect("web_search tool");
    assert_eq!(web["search_context_size"], json!("medium"));
    assert!(web.get("function").is_none(), "config must be top-level");

    let file = by_type("file_search").expect("file_search tool");
    assert_eq!(file["max_num_results"], json!(5));

    let computer = by_type("computer_use").expect("computer_use tool");
    assert_eq!(computer["display_width"], json!(1024));

    server.shutdown().await;
}

#[tokio::test]
async fn chat_hosted_tools_serialized_as_function_tools() {
    let server =
        MockLlmServer::start(|_req: &CapturedRequest| MockReply::Json(chat_completion("hi"))).await;
    let vendor = OpenAiVendor::new(server.url(), "gpt-4".into(), "sk-mock".into());

    let mut session = LlmSession::new();
    session.push(LlmMessage::user("hello"));
    let mut context = LlmContext::default();
    context.add_tool(LlmTool::web_search(None));
    let req = LlmRequest::new("gpt-4");

    vendor
        .complete(None, req, &mut session, &mut context)
        .await
        .expect("chat complete");

    let reqs = server.requests();
    let tools = reqs[0].body["tools"].as_array().expect("tools array");
    let fn_tool = &tools[0];
    assert_eq!(fn_tool["type"], json!("function"));
    assert_eq!(fn_tool["function"]["name"], json!("web_search"));
    server.shutdown().await;
}
