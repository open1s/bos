use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;

use agent::{
    Agent, AgentBuilder, AgentConfig, AgentError, LlmClient, LlmError, LlmRequest,
    LlmResponse, OpenAiMessage, StreamToken, Tool, ToolDescription, ToolError, ToolRegistry,
};

struct MockLlmClient {
    responses: Vec<LlmResponse>,
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn complete(&self, _req: LlmRequest) -> Result<LlmResponse, LlmError> {
        Ok(self.responses.clone().into_iter().next().unwrap_or(LlmResponse::Done))
    }

    fn stream_complete(
        &self,
        _req: LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send + '_>> {
        let responses = self.responses.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(32);

        tokio::spawn(async move {
            for response in responses {
                let token = match response {
                    LlmResponse::Text(s) => StreamToken::Text(s),
                    LlmResponse::ToolCall { name, args } => StreamToken::ToolCall { name, args },
                    LlmResponse::Done => StreamToken::Done,
                };
                let _ = tx.send(Ok(token)).await;
            }
        });

        Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))
    }

    fn supports_tools(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "mock"
    }
}

#[tokio::test]
async fn test_agent_single_turn() {
    let mock = MockLlmClient {
        responses: vec![LlmResponse::Text("Hello!".to_string()), LlmResponse::Done],
    };

    let config = AgentConfig {
        name: "test".to_string(),
        model: "gpt-4o".to_string(),
        base_url: "https://api.example.com".to_string(),
        api_key: "test-key".to_string(),
        system_prompt: "You are helpful.".to_string(),
        temperature: 0.7,
        max_tokens: None,
        timeout_secs: 60,
    };

    let mut agent = Agent::new(config, Arc::new(mock));
    let result = agent.run("Hi").await.unwrap();

    assert_eq!(result, "Hello!");
}

#[tokio::test]
async fn test_agent_with_tool_call() {
    struct AddTool;

    #[async_trait]
    impl Tool for AddTool {
        fn name(&self) -> &str {
            "add"
        }

        fn description(&self) -> ToolDescription {
            ToolDescription {
                short: "Add two numbers".to_string(),
                parameters: r#"{"type":"object","properties":{"a":{"type":"number"},"b":{"type":"number"}},"required":["a","b"]}"#.to_string(),
            }
        }

        fn json_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "a": {"type": "number"},
                    "b": {"type": "number"}
                },
                "required": ["a", "b"]
            })
        }

        async fn execute(&self, args: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
            let a = args["a"].as_f64().unwrap_or(0.0);
            let b = args["b"].as_f64().unwrap_or(0.0);
            Ok(serde_json::json!(a + b))
        }
    }

    let mock = MockLlmClient {
        responses: vec![
            LlmResponse::ToolCall {
                name: "add".to_string(),
                args: serde_json::json!({"a": 1, "b": 2}),
            },
            LlmResponse::Done,
        ],
    };

    let config = AgentConfig {
        name: "test".to_string(),
        model: "gpt-4o".to_string(),
        base_url: "https://api.example.com".to_string(),
        api_key: "test-key".to_string(),
        system_prompt: "You are helpful.".to_string(),
        temperature: 0.7,
        max_tokens: None,
        timeout_secs: 60,
    };

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(AddTool)).unwrap();

    let mut agent = Agent::new(config, Arc::new(mock));
    let result = agent.run_with_tools("Add 1 and 2", &registry).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_message_log_accumulation() {
    let mock = MockLlmClient {
        responses: vec![LlmResponse::Text("Response 1".to_string()), LlmResponse::Done],
    };

    let config = AgentConfig {
        name: "test".to_string(),
        model: "gpt-4o".to_string(),
        base_url: "https://api.example.com".to_string(),
        api_key: "test-key".to_string(),
        system_prompt: "You are helpful.".to_string(),
        temperature: 0.7,
        max_tokens: None,
        timeout_secs: 60,
    };

    let mut agent = Agent::new(config, Arc::new(mock));

    let initial_len = agent.message_log().len();

    agent.run("First task").await.unwrap();
    let len_after_first = agent.message_log().len();

    agent.run("Second task").await.unwrap();
    let len_after_second = agent.message_log().len();

    assert!(len_after_first > initial_len);
    assert!(len_after_second > len_after_first);
}

#[test]
fn test_agent_from_toml_config() {
    let toml_str = r#"
name = "test-agent"
model = "gpt-4o"
base_url = "https://api.example.com"
api_key = "test-key-123"
system_prompt = "You are a math assistant."
temperature = 0.5
max_tokens = 1000
timeout_secs = 30
"#;

    let builder = AgentBuilder::from_toml(toml_str).unwrap();
    assert_eq!(builder.config_tools(), None);
}

#[test]
fn test_agent_from_toml_config_with_tools() {
    let toml_str = r#"
name = "test-agent"
model = "gpt-4o"
base_url = "https://api.example.com"
api_key = "test-key-123"
system_prompt = "You are a math assistant."
temperature = 0.5

[[tools]]
name = "add"
description = "Add two numbers"
schema = {"type":"object","properties":{"a":{"type":"number"},"b":{"type":"number"}},"required":["a","b"]}
"#;

    let builder = AgentBuilder::from_toml(toml_str).unwrap();
    let tools = builder.config_tools();
    assert!(tools.is_some());

    let tools = tools.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["function"]["name"], "add");
}

#[tokio::test]
async fn test_tool_schema_validation() {
    struct AddTool;

    #[async_trait]
    impl Tool for AddTool {
        fn name(&self) -> &str {
            "add"
        }

        fn description(&self) -> ToolDescription {
            ToolDescription {
                short: "Add two numbers".to_string(),
                parameters: r#"{"type":"object","properties":{"a":{"type":"number"},"b":{"type":"number"}},"required":["a","b"]}"#.to_string(),
            }
        }

        fn json_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "a": {"type": "number"},
                    "b": {"type": "number"}
                },
                "required": ["a", "b"]
            })
        }

        async fn execute(&self, args: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
            let a = args["a"].as_f64().ok_or_else(|| {
                ToolError::SchemaMismatch("field 'a' is required".to_string())
            })?;
            let b = args["b"].as_f64().ok_or_else(|| {
                ToolError::SchemaMismatch("field 'b' is required".to_string())
            })?;
            Ok(serde_json::json!(a + b))
        }
    }

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(AddTool)).unwrap();

    let args = serde_json::json!({"a": "not a number", "b": 2});
    let result = registry.execute("add", &args).await;

    assert!(result.is_err());
}

#[test]
fn test_duplicate_tool_registration() {
    struct DummyTool;

    #[async_trait]
    impl Tool for DummyTool {
        fn name(&self) -> &str {
            "dummy"
        }

        fn description(&self) -> ToolDescription {
            ToolDescription {
                short: "A dummy tool".to_string(),
                parameters: "{}".to_string(),
            }
        }

        fn json_schema(&self) -> serde_json::Value {
            serde_json::json!({})
        }

        async fn execute(&self, _args: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
            Ok(serde_json::json!("executed"))
        }
    }

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(DummyTool)).unwrap();

    let result = registry.register(Arc::new(DummyTool));
    assert!(result.is_err());
}
