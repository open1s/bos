//! Core agent types: Message, MessageLog, Agent, AgentConfig.

pub mod agentic;
pub mod config;
pub mod context;

pub use agentic::{Agent, AgentBuilder, AgentConfig, AgentOutput};
pub use react::llm::{LlmMessage as Message, LlmRequest, LlmResponse, StreamToken};

fn format_tool_result_content(result: serde_json::Value) -> String {
    match result {
        serde_json::Value::String(content) => content,
        other => serde_json::to_string(&other).unwrap_or_else(|_| other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use crate::agent::agentic::{Agent, AgentConfig};
    use crate::tools::FunctionTool;
    use futures::Stream;
    use react::llm::{LlmClient, LlmRequest, LlmResponse, StreamToken};
    use serde_json::json;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio_stream::StreamExt;

    fn parse_calculator_args(args: &serde_json::Value) -> (f64, f64) {
        if let Some(arr) = args.as_array() {
            if arr.len() >= 2 {
                let a_val = arr[0]
                    .as_f64()
                    .or_else(|| arr[0].as_str().and_then(|s| s.parse().ok()))
                    .unwrap_or(0.0);
                let b_val = arr[1]
                    .as_f64()
                    .or_else(|| arr[1].as_str().and_then(|s| s.parse().ok()))
                    .unwrap_or(0.0);
                return (a_val, b_val);
            }
        } else if let Some(obj) = args.as_object() {
            let a_val = obj
                .get("a")
                .and_then(|v| {
                    v.as_f64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                })
                .unwrap_or(0.0);
            let b_val = obj
                .get("b")
                .and_then(|v| {
                    v.as_f64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                })
                .unwrap_or(0.0);
            return (a_val, b_val);
        }
        (0.0, 0.0)
    }

    fn is_external_api_error(message: &str) -> bool {
        let msg = message.to_ascii_lowercase();
        msg.contains("error sending request for url")
            || msg.contains("request failed")
            || msg.contains("connection")
            || msg.contains("timed out")
            || msg.contains("dns")
            || msg.contains("http(")
    }

    #[ctor::ctor]
    pub fn init() {
        std::env::set_var("RUST_BACKTRACE", "1");
        std::env::set_var("RUST_LOG", "debug");
        std::env::set_var(
            "API_KEY",
            "nvapi-xxx",
        );
    }

    struct MockLlm {
        response: String,
    }

    #[async_trait::async_trait]
    impl LlmClient for MockLlm {
        async fn complete(&self, _req: LlmRequest) -> Result<LlmResponse, react::llm::LlmError> {
            Ok(LlmResponse::Text(self.response.clone()))
        }

        async fn stream_complete(
            &self,
            _req: LlmRequest,
        ) -> Result<
            Pin<Box<dyn Stream<Item = Result<StreamToken, react::llm::LlmError>> + Send>>,
            react::llm::LlmError,
        > {
            let stream = futures::stream::iter(vec![
                Ok(StreamToken::Text(self.response.clone())),
                Ok(StreamToken::Done),
            ]);
            Ok(Box::pin(stream))
        }

        fn supports_tools(&self) -> bool {
            true
        }

        fn provider_name(&self) -> &'static str {
            "mock"
        }
    }

    #[tokio::test]
    async fn test_agent() {
        let response = "agent response";
        let llm = Arc::new(MockLlm {
            response: response.to_string(),
        });

        let config = AgentConfig {
            name: "test_agent".to_string(),
            model: "mock-model".to_string(),
            base_url: "http://localhost".to_string(),
            api_key: "test_key".to_string(),
            system_prompt: "You are a test assistant.".to_string(),
            temperature: 0.7,
            max_tokens: Some(100),
            timeout_secs: 60,
            rate_limit: None,
            context_compaction_threshold_tokens: 24_000,
            context_compaction_trigger_ratio: 0.85,
            context_compaction_keep_recent_messages: 12,
            context_compaction_max_summary_chars: 4_000,
            context_compaction_summary_max_tokens: 600,
        };

        let agent = Agent::new(config, llm);

        let result: String = agent.run_simple("Hello!").await.unwrap();
        println!("{}", result);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    #[tokio::test]
    async fn test_agent_stream() {
        let response = "agent response";
        let llm = Arc::new(MockLlm {
            response: response.to_string(),
        });

        let config = AgentConfig {
            name: "test_agent".to_string(),
            model: "mock-model".to_string(),
            base_url: "http://localhost".to_string(),
            api_key: "test_key".to_string(),
            system_prompt: "You are a test assistant.".to_string(),
            temperature: 0.7,
            max_tokens: Some(100),
            timeout_secs: 60,
            rate_limit: None,
            context_compaction_threshold_tokens: 24_000,
            context_compaction_trigger_ratio: 0.85,
            context_compaction_keep_recent_messages: 12,
            context_compaction_max_summary_chars: 4_000,
            context_compaction_summary_max_tokens: 600,
        };

        let mut agent = Agent::new(config, llm);

        let calculator_tool = Arc::new(FunctionTool::numeric(
            "add",
            "Add two numbers",
            2,
            |args: &serde_json::Value| {
                if let Some(arr) = args.as_array() {
                    if arr.len() >= 2 {
                        let a: f64 = arr[0].as_f64().unwrap_or(0.0);
                        let b: f64 = arr[1].as_f64().unwrap_or(0.0);
                        return Ok(json!(a + b));
                    }
                }
                Ok(json!(0))
            },
        ));

        agent.add_tool(calculator_tool);

        let mut stream = agent.stream("Hello!");
        while let Some(item) = stream.next().await {
            match item.unwrap() {
                StreamToken::Done => {
                    println!("Got done");
                }
                StreamToken::Text(token) => {
                    println!("{}", token);
                }
                StreamToken::ToolCall { name, .. } => {
                    println!("call {}", name);
                }
            }
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    #[tokio::test]
    async fn test_multiple_run_calls_panics() {
        let response = "agent response";
        let llm = Arc::new(MockLlm {
            response: response.to_string(),
        });

        let config = AgentConfig {
            name: "test_agent".to_string(),
            model: "mock-model".to_string(),
            base_url: "http://localhost".to_string(),
            api_key: "test_key".to_string(),
            system_prompt: "You are a test assistant.".to_string(),
            temperature: 0.7,
            max_tokens: None,
            timeout_secs: 30,
            rate_limit: None,
            context_compaction_threshold_tokens: 24_000,
            context_compaction_trigger_ratio: 0.85,
            context_compaction_keep_recent_messages: 12,
            context_compaction_max_summary_chars: 4_000,
            context_compaction_summary_max_tokens: 600,
        };

        let agent = Agent::new(config, llm);

        let result1 = agent.run_simple("Hello!").await.unwrap();
        println!("First call succeeded: {}", result1);

        let result2 = agent.run_simple("Hello again!").await.unwrap();
        println!("Second call succeeded: {}", result2);
    }

    #[tokio::test]
    async fn test_multiple_stream_run_calls_panics() {
        let response = "agent response";
        let llm = Arc::new(MockLlm {
            response: response.to_string(),
        });

        let config = AgentConfig {
            name: "test_agent".to_string(),
            model: "mock-model".to_string(),
            base_url: "http://localhost".to_string(),
            api_key: "test_key".to_string(),
            system_prompt: "You are a test assistant.".to_string(),
            temperature: 0.7,
            max_tokens: None,
            timeout_secs: 30,
            rate_limit: None,
            context_compaction_threshold_tokens: 24_000,
            context_compaction_trigger_ratio: 0.85,
            context_compaction_keep_recent_messages: 12,
            context_compaction_max_summary_chars: 4_000,
            context_compaction_summary_max_tokens: 600,
        };

        let agent = Agent::new(config, llm);

        let mut stream1 = agent.stream("Hello!");
        while let Some(item) = stream1.next().await {
            match item.unwrap() {
                StreamToken::Done => {
                    println!("Got done");
                }
                StreamToken::Text(token) => {
                    println!("{}", token);
                }
                StreamToken::ToolCall { name, .. } => {
                    println!("call {}", name);
                }
            }
        }
        println!("First stream_run succeeded");

        let mut stream2 = agent.stream("Hello again!");
        while let Some(item) = stream2.next().await {
            match item.unwrap() {
                StreamToken::Done => {
                    println!("Got done");
                }
                StreamToken::Text(token) => {
                    println!("{}", token);
                }
                StreamToken::ToolCall { name, .. } => {
                    println!("call {}", name);
                }
            }
        }
        println!("Second stream_run succeeded");
    }

    #[tokio::test]
    async fn test_multiple_run_tool_call_using_nvidia() {
        if std::env::var("API_KEY").is_err() {
            println!("API_KEY not set; skipping NVIDIA tool run test");
            return;
        }
        use react::llm::vendor::OpenAiClient;

        let api_key = std::env::var("API_KEY").unwrap();
        let llm = Arc::new(OpenAiClient::new(
            "https://integrate.api.nvidia.com/v1".to_string(),
            "meta/llama-3.1-8b-instruct".to_string(),
            api_key.to_string(),
        ));

        let config = AgentConfig {
            name: "test_agent".to_string(),
            model: "meta/llama-3.1-8b-instruct".to_string(),
            base_url: "https://integrate.api.nvidia.com/v1".to_string(),
            api_key: api_key.to_string(),
            system_prompt: "You are a helpful assistant that can use tools. When asked to perform calculations, use the add tool with NUMBER parameters (not strings). After receiving the tool result, provide the final answer to the user. For example, if asked 'What is 2 + 3?', call the add tool with parameters {\"a\": 2, \"b\": 3} (numbers, not strings), then respond with '2 + 3 is 5'.".to_string(),
            temperature: 0.7,
            max_tokens: Some(10000),
            timeout_secs: 30,
            rate_limit: None,
            context_compaction_threshold_tokens: 24_000,
            context_compaction_trigger_ratio: 0.85,
            context_compaction_keep_recent_messages: 12,
            context_compaction_max_summary_chars: 4_000,
            context_compaction_summary_max_tokens: 600,
        };

        let mut agent = Agent::new(config, llm);

        let calculator_tool = Arc::new(FunctionTool::numeric(
            "add",
            "Add two numbers",
            2,
            |args: &serde_json::Value| {
                let (a, b) = if let Some(arr) = args.as_array() {
                    if arr.len() >= 2 {
                        let a_val = arr[0]
                            .as_f64()
                            .or_else(|| arr[0].as_str().and_then(|s| s.parse().ok()))
                            .unwrap_or(0.0);
                        let b_val = arr[1]
                            .as_f64()
                            .or_else(|| arr[1].as_str().and_then(|s| s.parse().ok()))
                            .unwrap_or(0.0);
                        (a_val, b_val)
                    } else {
                        (0.0, 0.0)
                    }
                } else if let Some(obj) = args.as_object() {
                    let a_val = obj
                        .get("a")
                        .and_then(|v| {
                            v.as_f64()
                                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                        })
                        .unwrap_or(0.0);
                    let b_val = obj
                        .get("b")
                        .and_then(|v| {
                            v.as_f64()
                                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                        })
                        .unwrap_or(0.0);
                    (a_val, b_val)
                } else {
                    (0.0, 0.0)
                };
                Ok(json!(a + b))
            },
        ));

        agent.add_tool(calculator_tool.clone());

        println!("Starting first run...");
        let result1 = match agent.run_simple("What is 2 + 30?").await {
            Ok(r) => r,
            Err(e) => {
                if is_external_api_error(&format!("{:?}", e)) {
                    println!(
                        "Skipping NVIDIA tool run test due to external API error: {:?}",
                        e
                    );
                    return;
                }
                println!("First run error: {:?}", e);
                panic!("First run failed: {:?}", e);
            }
        };
        println!("First run result: {}", result1);
        assert!(
            result1.contains("32") || result1.contains("2 + 30"),
            "Expected result to contain '32' or '2 + 30', got: {}",
            result1
        );

        let llm2 = Arc::new(OpenAiClient::new(
            "https://integrate.api.nvidia.com/v1".to_string(),
            "meta/llama-3.1-8b-instruct".to_string(),
            api_key.to_string(),
        ));
        let config2 = AgentConfig {
            name: "test_agent".to_string(),
            model: "meta/llama-3.1-8b-instruct".to_string(),
            base_url: "https://integrate.api.nvidia.com/v1".to_string(),
            api_key: api_key.to_string(),
            system_prompt: "You are a helpful assistant that can use tools. When asked to perform calculations, use the add tool with NUMBER parameters (not strings). After receiving the tool result, provide the final answer to the user. For example, if asked 'What is 2 + 3?', call the add tool with parameters {\"a\": 2, \"b\": 3} (numbers, not strings), then respond with '2 + 3 is 5'.".to_string(),
            temperature: 0.7,
            max_tokens: Some(100),
            timeout_secs: 30,
            rate_limit: Default::default(),
            context_compaction_threshold_tokens: 24_000,
            context_compaction_trigger_ratio: 0.85,
            context_compaction_keep_recent_messages: 12,
            context_compaction_max_summary_chars: 4_000,
            context_compaction_summary_max_tokens: 600,
        };
        let mut agent2 = Agent::new(config2, llm2);
        agent2.add_tool(calculator_tool);

        println!("Starting second run...");
        let result2 = match agent2.run_simple("What is 50 + 7?").await {
            Ok(r) => r,
            Err(e) => {
                if is_external_api_error(&format!("{:?}", e)) {
                    println!(
                        "Skipping NVIDIA tool run test due to external API error: {:?}",
                        e
                    );
                    return;
                }
                panic!("Second run failed: {:?}", e);
            }
        };
        println!("Second run result: {}", result2);
        assert!(
            result2.contains("57") || result2.contains("50 + 7"),
            "Expected result to contain '57' or '50 + 7', got: {}",
            result2
        );

        let result2 = match agent2.run_simple("What is 50 + 70?").await {
            Ok(r) => r,
            Err(e) => {
                if is_external_api_error(&format!("{:?}", e)) {
                    println!(
                        "Skipping NVIDIA tool run test due to external API error: {:?}",
                        e
                    );
                    return;
                }
                println!("Second run error: {:?}", e);
                panic!("Second run failed: {:?}", e);
            }
        };

        println!("Third run result: {}", result2);
    }

    #[tokio::test]
    async fn test_multiple_stream_run_tool_call_using_nvidia() {
        if std::env::var("API_KEY").is_err() {
            println!("API_KEY not set; skipping NVIDIA tool call test");
            return;
        }
        use react::llm::vendor::OpenAiClient;

        let api_key = std::env::var("API_KEY").unwrap();
        let llm = Arc::new(OpenAiClient::new(
            "https://integrate.api.nvidia.com/v1".to_string(),
            "meta/llama-3.1-8b-instruct".to_string(),
            api_key.to_string(),
        ));

        let config = AgentConfig {
            name: "test_agent".to_string(),
            model: "meta/llama-3.1-8b-instruct".to_string(),
            base_url: "https://integrate.api.nvidia.com/v1".to_string(),
            api_key: api_key.to_string(),
            system_prompt: "You are a helpful assistant that can use tools. When asked to perform calculations, use the add tool with NUMBER parameters (not strings). After receiving the tool result, provide the final answer to the user. For example, if asked 'What is 2 + 3?', call the add tool with parameters {\"a\": 2, \"b\": 3} (numbers, not strings), then respond with '2 + 3 is 5'.".to_string(),
            temperature: 0.7,
            max_tokens: Some(100),
            timeout_secs: 30,
            rate_limit: None,
            context_compaction_threshold_tokens: 24_000,
            context_compaction_trigger_ratio: 0.85,
            context_compaction_keep_recent_messages: 12,
            context_compaction_max_summary_chars: 4_000,
            context_compaction_summary_max_tokens: 600,
        };

        let mut agent = Agent::new(config, llm);

        let calculator_tool = Arc::new(FunctionTool::numeric(
            "add",
            "Add two numbers",
            2,
            |args: &serde_json::Value| {
                let (a, b) = if let Some(arr) = args.as_array() {
                    if arr.len() >= 2 {
                        let a_val = arr[0]
                            .as_f64()
                            .or_else(|| arr[0].as_str().and_then(|s| s.parse().ok()))
                            .unwrap_or(0.0);
                        let b_val = arr[1]
                            .as_f64()
                            .or_else(|| arr[1].as_str().and_then(|s| s.parse().ok()))
                            .unwrap_or(0.0);
                        (a_val, b_val)
                    } else {
                        (0.0, 0.0)
                    }
                } else if let Some(obj) = args.as_object() {
                    let a_val = obj
                        .get("a")
                        .and_then(|v| {
                            v.as_f64()
                                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                        })
                        .unwrap_or(0.0);
                    let b_val = obj
                        .get("b")
                        .and_then(|v| {
                            v.as_f64()
                                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                        })
                        .unwrap_or(0.0);
                    (a_val, b_val)
                } else {
                    (0.0, 0.0)
                };
                Ok(json!(a + b))
            },
        ));

        agent.add_tool(calculator_tool.clone());

        println!("Starting first stream_run...");
        let mut stream1 = agent.stream("What is 2 + 30?");
        let mut result1 = String::new();
        while let Some(item) = stream1.next().await {
            match item {
                Ok(StreamToken::Done) => {
                    println!("Got done");
                }
                Ok(StreamToken::Text(token)) => {
                    result1.push_str(&token);
                }
                Ok(StreamToken::ToolCall { name, args, .. }) => {
                    println!("call {} {}", name, args);
                }
                Err(e) => {
                    if is_external_api_error(&format!("{:?}", e)) {
                        println!(
                            "Skipping NVIDIA stream tool test due to external API error: {:?}",
                            e
                        );
                        return;
                    }
                    panic!("Error during first stream_run: {:?}", e);
                }
            }
        }
        println!("First stream_run result: {}", result1);
        assert!(
            result1.contains("32") || result1.contains("2 + 30"),
            "Expected result to contain '32' or '2 + 30', got: {}",
            result1
        );

        let llm2 = Arc::new(OpenAiClient::new(
            "https://integrate.api.nvidia.com/v1".to_string(),
            "meta/llama-3.1-8b-instruct".to_string(),
            api_key.to_string(),
        ));

        let config2 = AgentConfig {
            name: "test_agent2".to_string(),
            model: "meta/llama-3.1-8b-instruct".to_string(),
            base_url: "https://integrate.api.nvidia.com/v1".to_string(),
            api_key: api_key.to_string(),
            system_prompt: "You are a helpful assistant that can use tools. When asked to perform calculations, use the add tool with NUMBER parameters (not strings). After receiving the tool result, provide the final answer to the user. For example, if asked 'What is 2 + 3?', call the add tool with parameters {\"a\": 2, \"b\": 3} (numbers, not strings), then respond with '2 + 3 is 5'.".to_string(),
            temperature: 0.7,
            max_tokens: Some(100),
            timeout_secs: 30,
            rate_limit: None,
            context_compaction_threshold_tokens: 24_000,
            context_compaction_trigger_ratio: 0.85,
            context_compaction_keep_recent_messages: 12,
            context_compaction_max_summary_chars: 4_000,
            context_compaction_summary_max_tokens: 600,
        };

        let mut agent2 = Agent::new(config2, llm2);
        agent2.add_tool(calculator_tool.clone());

        println!("Starting second stream...");
        let mut stream2 = agent2.stream("What is 50 + 7?");
        let mut result2 = String::new();
        while let Some(item) = stream2.next().await {
            match item {
                Ok(StreamToken::Done) => {
                    println!("Got done");
                }
                Ok(StreamToken::Text(token)) => {
                    result2.push_str(&token);
                }
                Ok(StreamToken::ToolCall { name, args, .. }) => {
                    println!("call {} {}", name, args);
                }
                Err(e) => {
                    if is_external_api_error(&format!("{:?}", e)) {
                        println!(
                            "Skipping NVIDIA stream tool test due to external API error: {:?}",
                            e
                        );
                        return;
                    }
                    panic!("Error during second stream_run: {:?}", e);
                }
            }
        }
        println!("Second stream_run result: {}", result2);
        assert!(
            result2.contains("57") || result2.contains("50 + 7"),
            "Expected result to contain '57' or '50 + 7', got: {}",
            result2
        );
    }

    #[tokio::test]
    async fn test_multiple_stream_run_mcp_using_nvidia() {
        if std::env::var("API_KEY").is_err() {
            println!("API_KEY not set; skipping NVIDIA MCP stream test");
            return;
        }
        use crate::mcp::McpClient;
        use react::llm::vendor::OpenAiClient;

        let api_key = std::env::var("API_KEY").unwrap();
        let llm = Arc::new(OpenAiClient::new(
            "https://integrate.api.nvidia.com/v1".to_string(),
            "meta/llama-3.1-8b-instruct".to_string(),
            api_key.to_string(),
        ));

        let config = AgentConfig {
            name: "test_agent".to_string(),
            model: "meta/llama-3.1-8b-instruct".to_string(),
            base_url: "https://integrate.api.nvidia.com/v1".to_string(),
            api_key: api_key.to_string(),
            system_prompt: "You are a helpful assistant that can use tools. When asked to perform calculations, use the add tool with NUMBER parameters (not strings). After receiving the tool result, provide the final answer to the user. For example, if asked 'What is 2 + 3?', call the add tool with parameters {\"a\": 2, \"b\": 3} (numbers, not strings), then respond with '2 + 3 is 5'.".to_string(),
            temperature: 0.7,
            max_tokens: Some(100),
            timeout_secs: 30,
            rate_limit: Default::default(),
            context_compaction_threshold_tokens: 24_000,
            context_compaction_trigger_ratio: 0.85,
            context_compaction_keep_recent_messages: 12,
            context_compaction_max_summary_chars: 4_000,
            context_compaction_summary_max_tokens: 600,
        };

        let mut agent = Agent::new(config, llm);

        let mock_server_path = std::path::PathBuf::from("tests/fixtures/mock_mcp_server.py");
        let client = Arc::new(
            McpClient::spawn("python3", &[mock_server_path.to_str().unwrap()])
                .await
                .unwrap(),
        );

        let _capabilities = client.initialize().await.unwrap();
        match agent.register_mcp_tools(client).await {
            Ok(_) => println!("MCP tools registered successfully"),
            Err(e) => {
                println!("Error details: {:?}", e);
                panic!("Failed to register MCP tools: {:?}", e);
            }
        }

        println!("Starting stream_run with MCP tool...");
        let mut stream = agent.stream("Use the echo tool upper to say hello");
        let mut result = String::new();
        let mut tool_called = false;
        while let Some(item) = stream.next().await {
            match item {
                Ok(StreamToken::Done) => {
                    println!("Got done");
                }
                Ok(StreamToken::Text(token)) => {
                    result.push_str(&token);
                }
                Ok(StreamToken::ToolCall { name, args, .. }) => {
                    println!("call {} {}", name, args);
                    tool_called = true;
                }
                Err(e) => {
                    if is_external_api_error(&format!("{:?}", e)) {
                        println!(
                            "Skipping NVIDIA MCP stream test due to external API error: {:?}",
                            e
                        );
                        return;
                    }
                    panic!("Error during stream: {:?}", e);
                }
            }
        }

        println!("Stream_run result: {}", result);
        println!("Tool was called: {}", tool_called);
    }

    #[tokio::test]
    async fn test_multiple_stream_run_skill_using_nvidia() {
        if std::env::var("API_KEY").is_err() {
            println!("API_KEY not set; skipping NVIDIA skill stream test");
            return;
        }
        use react::llm::vendor::OpenAiClient;

        let api_key = std::env::var("API_KEY").unwrap();
        let llm = Arc::new(OpenAiClient::new(
            "https://integrate.api.nvidia.com/v1".to_string(),
            "meta/llama-3.1-8b-instruct".to_string(),
            api_key.to_string(),
        ));

        let base_prompt = "You are a helpful assistant that can use tools. provide the correct final answer to the user.";

        let config = AgentConfig {
            name: "test_agent".to_string(),
            model: "meta/llama-3.1-8b-instruct".to_string(),
            base_url: "https://integrate.api.nvidia.com/v1".to_string(),
            api_key: api_key.to_string(),
            system_prompt: base_prompt.to_string(),
            temperature: 0.7,
            max_tokens: Some(100),
            timeout_secs: 30,
            rate_limit: None,
            context_compaction_threshold_tokens: 24_000,
            context_compaction_trigger_ratio: 0.85,
            context_compaction_keep_recent_messages: 12,
            context_compaction_max_summary_chars: 4_000,
            context_compaction_summary_max_tokens: 600,
        };

        let mut agent = Agent::new(config, llm);

        let skills_dir = std::path::PathBuf::from("tests/fixtures/skills");
        agent.register_skills_from_dir(skills_dir).unwrap();

        let skills_schemas = agent.get_skills_schemas();
        println!("Skills schemas: {:?}", skills_schemas);

        let calculator_tool = Arc::new(FunctionTool::numeric(
            "add",
            "Add two numbers",
            2,
            |args: &serde_json::Value| {
                let (a, b) = parse_calculator_args(args);
                Ok(json!(a + b))
            },
        ));

        let load_skill_tool = Arc::new(FunctionTool::new(
            "load_skill",
            "Load a skill by name to get its instructions",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name of the skill to load"
                    }
                },
                "required": ["name"]
            }),
            |args: &serde_json::Value| {
                let name = args
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                Ok(json!({ "loaded": name }))
            },
        ));

        agent.add_tool(calculator_tool);
        agent.add_tool(load_skill_tool);

        println!("Starting stream_run with skills...");
        let mut stream = agent.stream("What is 2 * 30?");
        let mut result = String::new();
        while let Some(item) = stream.next().await {
            match item {
                Ok(StreamToken::Done) => {
                    println!("Got done");
                }
                Ok(StreamToken::Text(token)) => {
                    result.push_str(&token);
                }
                Ok(StreamToken::ToolCall { name, args, .. }) => {
                    println!("call {} {}", name, args);
                }
                Err(e) => {
                    if is_external_api_error(&format!("{:?}", e)) {
                        println!(
                            "Skipping NVIDIA skill stream test due to external API error: {:?}",
                            e
                        );
                        return;
                    }
                    panic!("Error during stream: {:?}", e);
                }
            }
        }
        println!("Stream_run result What is 2 * 30?: {}", result);

        let has_sorry = result.to_lowercase().contains("sorry")
            && result.to_lowercase().contains("only perform addition");
        assert!(
            has_sorry,
            "Expected apology about limitation, got: {}",
            result
        );
    }
}
