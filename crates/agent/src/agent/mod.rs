//! Core agent types: Message, MessageLog, Agent, AgentConfig.

use std::sync::Arc;

pub mod config;

use crate::error::AgentError;
use crate::llm::{LlmClient, LlmRequest, LlmResponse, OpenAiMessage};
use crate::tools::ToolRegistry;

/// A message in the conversation history.
#[derive(Debug, Clone)]
pub enum Message {
    User(String),
    Assistant(String),
    ToolResult { name: String, content: String },
}

/// Conversation history wrapper.
#[derive(Debug, Clone, Default)]
pub struct MessageLog {
    messages: Vec<Message>,
}

impl MessageLog {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }

    pub fn add_user(&mut self, content: String) {
        self.messages.push(Message::User(content));
    }

    pub fn add_assistant(&mut self, content: String) {
        self.messages.push(Message::Assistant(content));
    }

    pub fn add_tool_result(&mut self, name: String, content: String) {
        self.messages.push(Message::ToolResult { name, content });
    }

    pub fn to_api_format(&self) -> Vec<OpenAiMessage> {
        self.messages
            .iter()
            .map(|m| match m {
                Message::User(content) => OpenAiMessage::User {
                    content: content.clone(),
                },
                Message::Assistant(content) => OpenAiMessage::Assistant {
                    content: content.clone(),
                },
                Message::ToolResult { name, content } => {
                    // tool_call_id uses the tool name as identifier
                    OpenAiMessage::ToolResult {
                        tool_call_id: name.clone(),
                        content: content.clone(),
                    }
                }
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

/// Agent configuration.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub name: String,
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    pub system_prompt: String,
    pub temperature: f32,
    pub max_tokens: Option<u32>,
    pub timeout_secs: u64,
}

/// Output from agent execution.
#[derive(Debug, Clone)]
pub enum AgentOutput {
    Text(String),
    Error(String),
}

/// The core agent struct.
pub struct Agent {
    config: AgentConfig,
    llm: Arc<dyn LlmClient>,
    message_log: MessageLog,
}

impl Agent {
    pub fn new(config: AgentConfig, llm: Arc<dyn LlmClient>) -> Self {
        Self {
            config,
            llm,
            message_log: MessageLog::new(),
        }
    }

    /// Run agent on a task (no tools).
    pub async fn run(&mut self, task: &str) -> Result<String, AgentError> {
        self.message_log.add_user(task.to_string());
        let output = self.run_loop(None).await?;
        match output {
            AgentOutput::Text(text) => Ok(text),
            AgentOutput::Error(e) => Err(AgentError::Session(e)),
        }
    }

    /// Run agent on a task with tools available.
    pub async fn run_with_tools(
        &mut self,
        task: &str,
        tools: &ToolRegistry,
    ) -> Result<AgentOutput, AgentError> {
        self.message_log.add_user(task.to_string());
        self.run_loop(Some(tools)).await
    }

    async fn run_loop(
        &mut self,
        tools: Option<&ToolRegistry>,
    ) -> Result<AgentOutput, AgentError> {
        const MAX_ITERATIONS: usize = 10;
        let mut accumulated_text = String::new();

        for _ in 0..MAX_ITERATIONS {
            let mut messages = vec![OpenAiMessage::System {
                content: self.config.system_prompt.clone(),
            }];
            messages.extend(self.message_log.to_api_format());

            let request = LlmRequest {
                model: self.config.model.clone(),
                messages,
                tools: tools.map(|t| t.to_openai_format()),
                temperature: self.config.temperature,
                max_tokens: self.config.max_tokens,
            };

            let response = self.llm.complete(request).await?;

            match response {
                LlmResponse::Text(text) => {
                    accumulated_text.push_str(&text);
                    self.message_log.add_assistant(text);
                }
                LlmResponse::ToolCall { name, args } => {
                    if let Some(registry) = tools {
                        let result = registry.execute(&name, args).await?;
                        self.message_log.add_tool_result(name, result.to_string());
                    }
                }
                LlmResponse::Done => break,
            }
        }

        Ok(AgentOutput::Text(accumulated_text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_log_to_api_format() {
        let mut log = MessageLog::new();
        log.add_user("Hello".to_string());
        log.add_assistant("Hi there!".to_string());
        log.add_tool_result("calculator".to_string(), "42".to_string());

        let api = log.to_api_format();
        assert_eq!(api.len(), 3);
        assert!(matches!(api[0], OpenAiMessage::User { .. }));
        assert!(matches!(api[1], OpenAiMessage::Assistant { .. }));
        assert!(matches!(api[2], OpenAiMessage::ToolResult { .. }));
    }

    #[test]
    fn test_message_log_len() {
        let mut log = MessageLog::new();
        assert!(log.is_empty());
        log.add_user("hi".to_string());
        assert_eq!(log.len(), 1);
    }
}
