use react::llm::vendor::{
    ChatCompletionResponse, ChatMessage, Choice, FunctionCall, OpenAiVendorBuilder, ToolCall,
};
use react::llm::{LlmClient, LlmResponse};

fn make_text_response(content: String, is_final: bool) -> LlmResponse {
    LlmResponse::OpenAI(ChatCompletionResponse {
        id: "test-123".to_string(),
        object: "chat.completion".to_string(),
        created: 1234567890,
        model: "test-model".to_string(),
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
            stop_reason: None,
            finish_reason: if is_final {
                Some("stop".to_string())
            } else {
                Some("continue".to_string())
            },
            logprobs: None,
        }],
        usage: None,
        system_fingerprint: None,
        nvext: None,
    })
}

fn make_tool_call_response(name: &str, args: serde_json::Value, call_id: &str) -> LlmResponse {
    LlmResponse::OpenAI(ChatCompletionResponse {
        id: "test-123".to_string(),
        object: "chat.completion".to_string(),
        created: 1234567890,
        model: "test-model".to_string(),
        choices: vec![Choice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: None,
                tool_calls: Some(vec![ToolCall {
                    id: call_id.to_string(),
                    r#type: "function".to_string(),
                    function: FunctionCall {
                        name: Some(name.to_string()),
                        arguments: Some(args.to_string()),
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
        usage: None,
        system_fingerprint: None,
        nvext: None,
    })
}

#[test]
fn test_openai_vendor_builder_new() {
    let builder = OpenAiVendorBuilder::new().api_key("test-key".to_string());
    let result = builder.build();
    assert!(result.is_ok());
}

#[test]
fn test_openai_vendor_builder_with_model() {
    let vendor = OpenAiVendorBuilder::new()
        .model("gpt-4".to_string())
        .api_key("test-key".to_string())
        .build()
        .expect("Should build vendor");

    assert!(vendor.supports_tools());
}

#[test]
fn test_openai_vendor_builder_with_endpoint() {
    let vendor = OpenAiVendorBuilder::new()
        .endpoint("https://api.openai.com/v1".to_string())
        .api_key("test-key".to_string())
        .build()
        .expect("Should build vendor");

    assert!(vendor.supports_tools());
}

#[test]
fn test_openai_vendor_builder_with_api_key() {
    let vendor = OpenAiVendorBuilder::new()
        .api_key("test-key-123".to_string())
        .build()
        .expect("Should build vendor");

    assert!(vendor.supports_tools());
}

#[test]
fn test_openai_vendor_builder_fluent() {
    let vendor = OpenAiVendorBuilder::new()
        .endpoint("https://api.openai.com/v1".to_string())
        .model("gpt-4".to_string())
        .api_key("sk-test123".to_string())
        .build()
        .expect("Should build vendor");

    assert!(vendor.supports_tools());
    assert_eq!(vendor.provider_name(), "openai");
}

#[test]
fn test_vendor_tool_response_parsing() {
    let response = make_tool_call_response("add", serde_json::json!({"a": 1, "b": 2}), "call-123");

    match response {
        LlmResponse::OpenAI(resp) => {
            let choice = &resp.choices[0];
            let tc = choice.message.tool_calls.as_ref().unwrap();
            assert_eq!(tc[0].function.name.as_ref().unwrap(), "add");
        }
    }
}

#[test]
fn test_vendor_text_response_parsing() {
    let response = make_text_response("Hello world".to_string(), true);

    match response {
        LlmResponse::OpenAI(resp) => {
            let content = resp.choices[0].message.content.as_ref().unwrap();
            assert_eq!(content, "Hello world");
        }
    }
}

#[test]
fn test_vendor_finish_reason_stop() {
    let response = make_text_response("done".to_string(), true);

    match &response {
        LlmResponse::OpenAI(resp) => {
            assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("stop"));
        }
    }
}

#[test]
fn test_vendor_finish_reason_continue() {
    let response = make_text_response("more coming".to_string(), false);

    match &response {
        LlmResponse::OpenAI(resp) => {
            assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("continue"));
        }
    }
}

#[test]
fn test_vendor_finish_reason_tool_calls() {
    let response = make_tool_call_response("calc", serde_json::json!({}), "call-1");

    match &response {
        LlmResponse::OpenAI(resp) => {
            assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("tool_calls"));
        }
    }
}

#[tokio::test]
async fn test_vendor_clonable() {
    let vendor = OpenAiVendorBuilder::new()
        .model("gpt-4".to_string())
        .endpoint("https://api.openai.com/v1".to_string())
        .api_key("test-key".to_string())
        .build()
        .expect("Should build");

    let cloned = vendor.clone();
    assert_eq!(cloned.provider_name(), vendor.provider_name());
}
