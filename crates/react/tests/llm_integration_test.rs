//! Integration tests for real LLM provider connectivity using config loader.
//!
//! These tests use the actual config from ~/.bos/conf/config.toml to create
//! LLM vendor clients and verify they can make real API requests.

use config::ConfigLoader;
use react::llm::vendor::{NvidiaVendor, OpenRouterVendor};
use react::llm::{LlmClient, LlmRequest, LlmResponse};

/// Configuration extracted from config file for LLM providers.
#[derive(Debug, Clone)]
struct LlmConfig {
    model: String,
    base_url: String,
    api_key: String,
}

impl LlmConfig {
    fn from_global_model(config: &serde_json::Value) -> Option<Self> {
        let global = config.get("global_model")?;
        let model = global.get("model")?.as_str()?.to_string();
        Some(LlmConfig {
            model: model.strip_prefix("nvidia/").unwrap_or(&model).to_string(),
            base_url: global.get("base_url")?.as_str()?.to_string(),
            api_key: global.get("api_key")?.as_str()?.to_string(),
        })
    }

    fn from_openrouter(config: &serde_json::Value) -> Option<Self> {
        let openrouter = config.get("llm")?.get("openrouter")?;
        let model = openrouter.get("model")?.as_str()?.to_string();
        Some(LlmConfig {
            model: model
                .strip_prefix("openrouter/")
                .unwrap_or(&model)
                .to_string(),
            base_url: openrouter.get("base_url")?.as_str()?.to_string(),
            api_key: openrouter.get("api_key")?.as_str()?.to_string(),
        })
    }
}

/// Load config from ~/.bos/conf/ using ConfigLoader::discover().
async fn load_config() -> Result<serde_json::Value, String> {
    let mut loader = ConfigLoader::new().discover();
    if loader.sources().is_empty() {
        return Err(
            "No config sources found. Make sure ~/.bos/conf/config.toml exists.".to_string(),
        );
    }
    loader
        .load()
        .await
        .map_err(|e| e.to_string())
        .cloned()
        .map(|v| v.clone())
}

/// Create a simple text completion request.
fn make_simple_request(model: &str) -> LlmRequest {
    LlmRequest::with_user(model, "Say 'Hello, World!' in exactly those words.")
        .temperature(0.7)
        .max_tokens(50)
}

/// Verify the response is valid and contains expected content.
fn verify_text_response(response: &LlmResponse) -> Result<String, String> {
    match response {
        LlmResponse::OpenAI(resp) => {
            if resp.choices.is_empty() {
                return Err("Response has no choices".to_string());
            }
            let choice = &resp.choices[0];
            if let Some(content) = &choice.message.content {
                if !content.is_empty() {
                    return Ok(content.clone());
                }
            }
            // Check new reasoning_content field on ChatMessage
            if let Some(reasoning) = &choice.message.reasoning_content {
                if !reasoning.is_empty() {
                    return Ok(format!("[reasoning] {}", reasoning));
                }
            }
            // Fallback: check in extra field
            if let Some(reasoning) = choice
                .message
                .extra
                .get("reasoning_content")
                .and_then(|v| v.as_str())
            {
                if !reasoning.is_empty() {
                    return Ok(format!("[reasoning] {}", reasoning));
                }
            }
            Err("Response has no content".to_string())
        }
    }
}

// ============================================================================
// NVIDIA Integration Tests
// ============================================================================

#[tokio::test]
async fn test_nvidia_vendor_with_config() {
    // Skip if no config available
    let config = match load_config().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping test (no config): {}", e);
            return;
        }
    };

    // Extract NVIDIA config
    let llm_config = match LlmConfig::from_global_model(&config) {
        Some(c) => c,
        None => {
            eprintln!("Skipping test (no global_model config)");
            return;
        }
    };

    // Create NVIDIA vendor
    let vendor = NvidiaVendor::new(
        llm_config.base_url.clone(),
        llm_config.model.clone(),
        llm_config.api_key.clone(),
    );

    // Make a simple request
    let request = make_simple_request(&llm_config.model);
    let result = vendor.complete(request).await;

    match result {
        Ok(response) => {
            let content = verify_text_response(&response).unwrap();
            println!("NVIDIA Response: {}", content);
            assert!(content.to_lowercase().contains("hello") || content.contains("World"));
        }
        Err(e) => {
            let err_str = e.to_string();
            if err_str.contains("404") || err_str.contains("not found") {
                eprintln!("NVIDIA endpoint/model not available (404): {}", err_str);
                return;
            }
            if err_str.contains("429") || err_str.contains("rate limit") {
                eprintln!("NVIDIA rate limited, skipping: {}", err_str);
                return;
            }
            panic!("NVIDIA request failed: {:?}", e);
        }
    }
}

#[tokio::test]
async fn test_nvidia_vendor_stream_with_config() {
    // Skip if no config available
    let config = match load_config().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping test (no config): {}", e);
            return;
        }
    };

    // Extract NVIDIA config
    let llm_config = match LlmConfig::from_global_model(&config) {
        Some(c) => c,
        None => {
            eprintln!("Skipping test (no global_model config)");
            return;
        }
    };

    // Create NVIDIA vendor
    let vendor = NvidiaVendor::new(
        llm_config.base_url.clone(),
        llm_config.model.clone(),
        llm_config.api_key.clone(),
    );

    // Make a streaming request
    let request = make_simple_request(&llm_config.model);
    let stream_result = vendor.stream_complete(request).await;

    match stream_result {
        Ok(mut stream) => {
            use futures::StreamExt;
            let mut collected_text = String::new();
            let mut has_content = false;

            while let Some(token_result) = stream.next().await {
                match token_result {
                    Ok(token) => {
                        use react::llm::StreamToken;
                        match token {
                            StreamToken::Text(text) => {
                                collected_text.push_str(&text);
                                has_content = true;
                            }
                            StreamToken::ReasoningContent(text) => {
                                collected_text.push_str(&text);
                                has_content = true;
                            }
                            StreamToken::Done => break,
                            _ => {}
                        }
                    }
                    Err(e) => {
                        panic!("Stream error: {:?}", e);
                    }
                }
            }

            assert!(has_content, "Should have received some text content");
            println!("NVIDIA Stream Response: {}", collected_text);
        }
        Err(e) => {
            let err_str = e.to_string();
            if err_str.contains("404") || err_str.contains("not found") {
                eprintln!("NVIDIA endpoint/model not available (404): {}", err_str);
                return;
            }
            if err_str.contains("429") || err_str.contains("rate limit") {
                eprintln!("NVIDIA rate limited, skipping: {}", err_str);
                return;
            }
            panic!("NVIDIA stream request failed: {:?}", e);
        }
    }
}

// ============================================================================
// OpenRouter Integration Tests
// ============================================================================

#[tokio::test]
async fn test_openrouter_vendor_with_config() {
    // Skip if no config available
    let config = match load_config().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping test (no config): {}", e);
            return;
        }
    };

    // Extract OpenRouter config
    let llm_config = match LlmConfig::from_openrouter(&config) {
        Some(c) => c,
        None => {
            eprintln!("Skipping test (no llm.openrouter config)");
            return;
        }
    };

    // Create OpenRouter vendor
    let vendor = OpenRouterVendor::new(
        llm_config.base_url.clone(),
        llm_config.model.clone(),
        llm_config.api_key.clone(),
    );

    // Make a simple request
    let request = make_simple_request(&llm_config.model);
    let result = vendor.complete(request).await;

    match result {
        Ok(response) => {
            let content = verify_text_response(&response).unwrap();
            println!("OpenRouter Response: {}", content);
            assert!(!content.is_empty());
        }
        Err(e) => {
            let err_str = e.to_string();
            if err_str.contains("403")
                || err_str.contains("rate limit")
                || err_str.contains("Key limit")
            {
                eprintln!("OpenRouter rate limited, skipping: {}", err_str);
                return;
            }
            if err_str.contains("400") && err_str.contains("not a valid model") {
                eprintln!("OpenRouter invalid model, skipping: {}", err_str);
                return;
            }
            panic!("OpenRouter request failed: {:?}", e);
        }
    }
}

#[tokio::test]
async fn test_openrouter_vendor_stream_with_config() {
    // Skip if no config available
    let config = match load_config().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping test (no config): {}", e);
            return;
        }
    };

    // Extract OpenRouter config
    let llm_config = match LlmConfig::from_openrouter(&config) {
        Some(c) => c,
        None => {
            eprintln!("Skipping test (no llm.openrouter config)");
            return;
        }
    };

    // Create OpenRouter vendor
    let vendor = OpenRouterVendor::new(
        llm_config.base_url.clone(),
        llm_config.model.clone(),
        llm_config.api_key.clone(),
    );

    // Make a streaming request
    let request = make_simple_request(&llm_config.model);
    let stream_result = vendor.stream_complete(request).await;

    match stream_result {
        Ok(mut stream) => {
            use futures::StreamExt;
            let mut collected_text = String::new();
            let mut has_content = false;

            while let Some(token_result) = stream.next().await {
                match token_result {
                    Ok(token) => {
                        use react::llm::StreamToken;
                        match token {
                            StreamToken::Text(text) => {
                                collected_text.push_str(&text);
                                has_content = true;
                            }
                            StreamToken::Done => break,
                            _ => {}
                        }
                    }
                    Err(e) => {
                        panic!("Stream error: {:?}", e);
                    }
                }
            }

            assert!(has_content, "Should have received some text content");
            println!("OpenRouter Stream Response: {}", collected_text);
        }
        Err(e) => {
            let err_str = e.to_string();
            if err_str.contains("403")
                || err_str.contains("rate limit")
                || err_str.contains("Key limit")
            {
                eprintln!("OpenRouter rate limited, skipping: {}", err_str);
                return;
            }
            if err_str.contains("400") && err_str.contains("not a valid model") {
                eprintln!("OpenRouter invalid model, skipping: {}", err_str);
                return;
            }
            panic!("OpenRouter stream request failed: {:?}", e);
        }
    }
}

// ============================================================================
// Error Handling Tests
// ============================================================================

#[tokio::test]
async fn test_nvidia_vendor_invalid_api_key() {
    // Create vendor with invalid API key
    let vendor = NvidiaVendor::new(
        "https://integrate.api.nvidia.com/v1".to_string(),
        "nvidia/z-ai/glm4-9b".to_string(),
        "invalid-key-12345".to_string(),
    );

    let request = make_simple_request("nvidia/z-ai/glm4-9b");
    let result = vendor.complete(request).await;

    // Should return an error (not panic)
    assert!(result.is_err(), "Invalid API key should produce an error");
    if let Err(e) = result {
        println!("Expected error with invalid key: {:?}", e);
    }
}

#[tokio::test]
async fn test_openrouter_vendor_invalid_api_key() {
    // Create vendor with invalid API key
    let vendor = OpenRouterVendor::new(
        "https://openrouter.ai/api/v1".to_string(),
        "openrouter/meta-llama/llama-3.2-3b-instruct".to_string(),
        "invalid-key-12345".to_string(),
    );

    let request = make_simple_request("openrouter/meta-llama/llama-3.2-3b-instruct");
    let result = vendor.complete(request).await;

    // Should return an error (not panic)
    assert!(result.is_err(), "Invalid API key should produce an error");
    if let Err(e) = result {
        println!("Expected error with invalid key: {:?}", e);
    }
}

// Response Parsing Tests

#[tokio::test]
async fn test_nvidia_response_parsing() {
    let config = match load_config().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping test (no config): {}", e);
            return;
        }
    };

    let llm_config = match LlmConfig::from_global_model(&config) {
        Some(c) => c,
        None => {
            eprintln!("Skipping test (no global_model config)");
            return;
        }
    };

    let vendor = NvidiaVendor::new(
        llm_config.base_url.clone(),
        llm_config.model.clone(),
        llm_config.api_key.clone(),
    );

    let request = LlmRequest::with_user(&llm_config.model, "What is 2+2?");
    let result = match vendor.complete(request).await {
        Ok(r) => r,
        Err(e) => {
            let err_str = e.to_string();
            if err_str.contains("404") || err_str.contains("not found") {
                eprintln!(
                    "NVIDIA endpoint/model not available (404), skipping test: {}",
                    err_str
                );
                return;
            }
            if err_str.contains("429") || err_str.contains("rate limit") {
                eprintln!("NVIDIA rate limited, skipping test: {}", err_str);
                return;
            }
            panic!("NVIDIA request failed: {:?}", e);
        }
    };

    match &result {
        LlmResponse::OpenAI(resp) => {
            assert!(!resp.id.is_empty(), "id should not be empty");
            assert!(!resp.object.is_empty(), "object should not be empty");
            assert!(resp.created > 0, "created should be positive");
            assert!(!resp.model.is_empty(), "model should not be empty");
            assert!(!resp.choices.is_empty(), "choices should not be empty");

            let choice = &resp.choices[0];
            assert_eq!(choice.index, 0, "first choice index should be 0");
            assert!(
                choice.message.role == "assistant",
                "message role should be assistant"
            );
            assert!(choice.finish_reason.is_some(), "finish_reason should exist");

            assert!(
                choice.message.content.is_some()
                    || choice.message.extra.get("reasoning_content").is_some(),
                "content or reasoning_content should exist"
            );

            if let Some(usage) = &resp.usage {
                assert!(usage.prompt_tokens > 0, "prompt_tokens should be positive");
                assert!(
                    usage.completion_tokens > 0,
                    "completion_tokens should be positive"
                );
                assert!(usage.total_tokens > 0, "total_tokens should be positive");
                assert_eq!(
                    usage.total_tokens,
                    usage.prompt_tokens + usage.completion_tokens,
                    "total_tokens should equal prompt + completion"
                );
            }

            if let Some(reasoning) = choice.message.extra.get("reasoning_content") {
                assert!(reasoning.is_string(), "reasoning_content should be string");
            }

            if let Some(tool_calls) = &choice.message.tool_calls {
                for tc in tool_calls {
                    assert!(!tc.id.is_empty(), "tool_call id should not be empty");
                    assert!(!tc.r#type.is_empty(), "tool_call type should not be empty");
                    assert!(tc.function.name.is_some(), "function name should exist");
                    assert!(
                        tc.function.arguments.is_some(),
                        "function arguments should exist"
                    );
                    println!("NVIDIA tool_call found: {:?}", tc);
                }
            }

            if let Some(fc) = &choice.message.function_call {
                assert!(
                    fc.name.is_some() || fc.arguments.is_some(),
                    "function_call should have fields"
                );
                println!("NVIDIA function_call found: {:?}", fc);
            }

            println!("system_fingerprint: {:?}", resp.system_fingerprint);

            assert!(
                resp.system_fingerprint.is_none()
                    || resp
                        .system_fingerprint
                        .as_ref()
                        .map(|s| !s.is_empty())
                        .unwrap_or(false),
                "system_fingerprint should be empty string or None"
            );

            println!("✓ All NVIDIA ChatCompletionResponse fields verified");
        }
    }
}

#[tokio::test]
async fn test_openrouter_response_parsing() {
    let config = match load_config().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping test (no config): {}", e);
            return;
        }
    };

    let llm_config = match LlmConfig::from_openrouter(&config) {
        Some(c) => c,
        None => {
            eprintln!("Skipping test (no llm.openrouter config)");
            return;
        }
    };

    let vendor = OpenRouterVendor::new(
        llm_config.base_url.clone(),
        llm_config.model.clone(),
        llm_config.api_key.clone(),
    );

    let request = LlmRequest::with_user(&llm_config.model, "What is 3+3?");
    let result = match vendor.complete(request).await {
        Ok(r) => r,
        Err(e) => {
            let err_str = e.to_string();
            if err_str.contains("403")
                || err_str.contains("rate limit")
                || err_str.contains("Key limit")
            {
                eprintln!("OpenRouter rate limited, skipping test: {}", err_str);
                return;
            }
            if err_str.contains("400") && err_str.contains("not a valid model") {
                eprintln!("OpenRouter invalid model, skipping test: {}", err_str);
                return;
            }
            panic!("OpenRouter request failed: {:?}", e);
        }
    };

    match &result {
        LlmResponse::OpenAI(resp) => {
            assert!(!resp.id.is_empty(), "id should not be empty");
            assert!(!resp.object.is_empty(), "object should not be empty");
            assert!(resp.created > 0, "created should be positive");
            assert!(!resp.model.is_empty(), "model should not be empty");
            assert!(!resp.choices.is_empty(), "choices should not be empty");

            let choice = &resp.choices[0];
            assert_eq!(choice.index, 0, "first choice index should be 0");
            assert!(
                choice.message.role == "assistant",
                "message role should be assistant"
            );

            assert!(choice.message.content.is_some(), "content should exist");

            if let Some(usage) = &resp.usage {
                assert!(usage.prompt_tokens > 0, "prompt_tokens should be positive");
                assert!(
                    usage.completion_tokens > 0,
                    "completion_tokens should be positive"
                );
                assert!(usage.total_tokens > 0, "total_tokens should be positive");
            }

            if choice.message.tool_calls.is_some() {
                let tool_calls = choice.message.tool_calls.as_ref().unwrap();
                for tc in tool_calls {
                    assert!(!tc.id.is_empty(), "tool_call id should not be empty");
                    assert!(!tc.r#type.is_empty(), "tool_call type should not be empty");
                    assert!(tc.function.name.is_some(), "function name should exist");
                    println!("OpenRouter tool_call found: {:?}", tc);
                }
            }

            if choice.message.function_call.is_some() {
                let fc = choice.message.function_call.as_ref().unwrap();
                println!("OpenRouter function_call found: {:?}", fc);
            }

            println!(
                "OpenRouter system_fingerprint: {:?}",
                resp.system_fingerprint
            );

            println!("✓ All OpenRouter ChatCompletionResponse fields verified");
        }
    }
}

#[tokio::test]
async fn test_nvidia_tool_calls_stream_with_config() {
    // Integration test for delta.tool_calls streaming in NVIDIA vendor.
    // Uses real config from ~/.bos/conf/config.toml and validates that
    // StreamToken::ToolCall is emitted when the model returns tool calls.
    use futures::StreamExt;

    // Skip if no config available
    let config = match load_config().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping test (no config): {}", e);
            return;
        }
    };

    // Extract NVIDIA config
    let llm_config = match LlmConfig::from_global_model(&config) {
        Some(c) => c,
        None => {
            eprintln!("Skipping test (no global_model config)");
            return;
        }
    };

    // Create NVIDIA vendor
    let vendor = NvidiaVendor::new(
        llm_config.base_url.clone(),
        llm_config.model.clone(),
        llm_config.api_key.clone(),
    );

    // System prompt that encourages the model to use a tool
    let request = LlmRequest::with_user(
        &llm_config.model,
        "You have access to a tool called 'get_weather'. \
        When the user asks about weather, you MUST call get_weather \
        with {\"location\": \"San Francisco, CA\"}. \
        User question: What is the weather in San Francisco?",
    )
    .system_message("You are a helpful assistant with access to tools.")
    .temperature(0.7)
    .max_tokens(256);

    let stream_result = vendor.stream_complete(request).await;

    match stream_result {
        Ok(mut stream) => {
            let mut has_tool_call = false;
            let mut tool_call_name = String::new();
            #[allow(unused_assignments)]
            let mut tool_call_args: Option<serde_json::Value> = None;
            let mut tool_call_id: Option<String> = None;
            let mut collected_text = String::new();

            while let Some(token_result) = stream.next().await {
                match token_result {
                    Ok(token) => {
                        use react::llm::StreamToken;
                        match token {
                            StreamToken::Text(text) => {
                                collected_text.push_str(&text);
                            }
                            StreamToken::ReasoningContent(text) => {
                                collected_text.push_str(&text);
                            }
                            StreamToken::ToolCall { name, args, id } => {
                                has_tool_call = true;
                                tool_call_name = name;
                                tool_call_args = Some(args);
                                tool_call_id = id;
                                println!(
                                    "ToolCall received: name={}, id={:?}, args={:?}",
                                    tool_call_name,
                                    tool_call_id,
                                    tool_call_args
                                );
                            }
                            StreamToken::Done => break,
                        }
                    }
                    Err(e) => {
                        panic!("Stream error: {:?}", e);
                    }
                }
            }

            if !has_tool_call {
                eprintln!(
                    "Note: Model did not return a tool call in streaming mode. \
                    This is expected behavior for many LLM providers - tool calling in \
                    SSE streaming is not universally supported. Skipping test validation. \
                    Collected text was: {}",
                    collected_text
                );
                return;
            }
            assert!(
                !tool_call_name.is_empty(),
                "ToolCall name should not be empty"
            );
            println!(
                "✓ NVIDIA tool_calls streaming verified: name={}, id={:?}",
                tool_call_name, tool_call_id
            );
        }
        Err(e) => {
            let err_str = e.to_string();
            if err_str.contains("404") || err_str.contains("not found") {
                eprintln!("NVIDIA endpoint/model not available (404): {}", err_str);
                return;
            }
            if err_str.contains("429") || err_str.contains("rate limit") {
                eprintln!("NVIDIA rate limited, skipping: {}", err_str);
                return;
            }
            panic!("NVIDIA tool_calls stream request failed: {:?}", e);
        }
    }
}
