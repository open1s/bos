//! Core agent types: Message, MessageLog, Agent, AgentConfig.
#[allow(dead_code)]
use futures::Stream;

pub mod config;
pub mod context;
pub mod message;
pub mod agentic;

pub use crate::llm::{LlmRequest, LlmResponse, StreamToken};
pub use agentic::{Agent,AgentConfig,AgentOutput};


fn format_tool_result_content(result: serde_json::Value) -> String {
    match result {
        serde_json::Value::String(content) => content,
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use crate::agent::agentic::{Agent, AgentConfig};
    use crate::llm::{LlmClient, LlmRequest, LlmResponse, StreamToken};
    use crate::tools::FunctionTool;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio_stream::StreamExt;

    struct MockLlm {
        response: String,
    }

    #[async_trait::async_trait]
    impl LlmClient for MockLlm {
        async fn complete(&self, _req: LlmRequest) -> Result<LlmResponse, crate::llm::LlmError> {
            Ok(LlmResponse::Text(self.response.clone()))
        }

        fn stream_complete(
            &self,
            _req: LlmRequest,
        ) -> Pin<Box<dyn Stream<Item = Result<StreamToken, crate::llm::LlmError>> + Send + '_>> {
            let stream = futures::stream::iter(vec![
                Ok(StreamToken::Text(self.response.clone())),
                //TODO: DEMO call add tool request here                   s
                Ok(StreamToken::Done),
            ]);
            Box::pin(stream)
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
        let llm = Arc::new(MockLlm { response: response.to_string() });

        let config = AgentConfig {
            name: "test_agent".to_string(),
            model: "mock-model".to_string(),
            base_url: "http://localhost".to_string(),
            api_key: "test_key".to_string(),
            system_prompt: "You are a test assistant.".to_string(),
            temperature: 0.7,
            max_tokens: Some(100),
            timeout_secs: 60,
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

        let result = agent.run("Hello!").await.unwrap();
        println!("{}",result);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    #[tokio::test]
    async fn test_agent_stream() {
        let response = "agent response";
        let llm = Arc::new(MockLlm { response: response.to_string() });

        let config = AgentConfig {
            name: "test_agent".to_string(),
            model: "mock-model".to_string(),
            base_url: "http://localhost".to_string(),
            api_key: "test_key".to_string(),
            system_prompt: "You are a test assistant.".to_string(),
            temperature: 0.7,
            max_tokens: Some(100),
            timeout_secs: 60,
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

        let mut stream = agent.stream_run("Hello!");
        while let Some(item) = stream.next().await {
            match item.unwrap() {
                StreamToken::Done => {
                    println!("Got done");
                },
                StreamToken::Text(token) => {
                    println!("{}", token);
                },
                StreamToken::ToolCall { name,.. } =>{
                    println!("call {}", name);
                }
            }
        }


        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    #[tokio::test]
    async fn test_multiple_run_calls_panics() {
        let response = "agent response";
        let llm = Arc::new(MockLlm { response: response.to_string() });

        let config = AgentConfig {
            name: "test_agent".to_string(),
            model: "mock-model".to_string(),
            base_url: "http://localhost".to_string(),
            api_key: "test_key".to_string(),
            system_prompt: "You are a test assistant.".to_string(),
            temperature: 0.7,
            max_tokens: None,
            timeout_secs: 30,
        };

        let mut agent = Agent::new(config, llm);

        let result1 = agent.run("Hello!").await.unwrap();
        println!("First call succeeded: {}", result1);

        let result2 = agent.run("Hello again!").await.unwrap();
        println!("Second call succeeded: {}", result2);
    }

    #[tokio::test]
    async fn test_multiple_stream_run_calls_panics() {
        let response = "agent response";
        let llm = Arc::new(MockLlm { response: response.to_string() });

        let config = AgentConfig {
            name: "test_agent".to_string(),
            model: "mock-model".to_string(),
            base_url: "http://localhost".to_string(),
            api_key: "test_key".to_string(),
            system_prompt: "You are a test assistant.".to_string(),
            temperature: 0.7,
            max_tokens: None,
            timeout_secs: 30,
        };

        let mut agent = Agent::new(config, llm);

        let mut stream1 = agent.stream_run("Hello!");
        while let Some(item) = stream1.next().await {
            match item.unwrap() {
                StreamToken::Done => {
                    println!("Got done");
                },
                StreamToken::Text(token) => {
                    println!("{}", token);
                },
                StreamToken::ToolCall { name,.. } =>{
                    println!("call {}", name);
                }
            }
        }
        println!("First stream_run succeeded");

        let mut stream2 = agent.stream_run("Hello again!");
        while let Some(item) = stream2.next().await {
            match item.unwrap() {
                StreamToken::Done => {
                    println!("Got done");
                },
                StreamToken::Text(token) => {
                    println!("{}", token);
                },
                StreamToken::ToolCall { name,.. } =>{
                    println!("call {}", name);
                }
            }
        }
        println!("Second stream_run succeeded");
    }

    #[tokio::test]
    async fn test_multiple_stream_run_tool_call_using_nvidia() {

    }
}
