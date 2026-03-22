//! Core agent types: Message, MessageLog, Agent, AgentConfig.

use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use futures::StreamExt;
use serde::{Deserialize, Serialize};

pub mod config;

use crate::error::AgentError;
use crate::llm::{LlmClient, LlmRequest, LlmResponse, OpenAiMessage, StreamToken};
use crate::tools::ToolRegistry;

/// A message in the conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

    pub fn append_assistant_chunk(&mut self, chunk: &str) {
        match self.messages.last_mut() {
            Some(Message::Assistant(content)) => content.push_str(chunk),
            _ => self.messages.push(Message::Assistant(chunk.to_string())),
        }
    }

    pub fn add_tool_result(&mut self, name: String, content: String) {
        self.messages.push(Message::ToolResult { name, content });
    }

    pub fn to_api_format(&self) -> Vec<OpenAiMessage> {
        let mut api_messages = Vec::with_capacity(self.messages.len());
        self.extend_api_format(&mut api_messages);
        api_messages
    }

    pub fn extend_api_format(&self, target: &mut Vec<OpenAiMessage>) {
        target.reserve(self.messages.len());
        for message in &self.messages {
            target.push(match message {
                Message::User(content) => OpenAiMessage::User {
                    content: content.clone(),
                },
                Message::Assistant(content) => OpenAiMessage::Assistant {
                    content: content.clone(),
                },
                Message::ToolResult { name, content } => OpenAiMessage::ToolResult {
                    tool_call_id: name.clone(),
                    content: content.clone(),
                },
            });
        }
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

fn format_tool_result_content(result: serde_json::Value) -> String {
    match result {
        serde_json::Value::String(content) => content,
        other => other.to_string(),
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

    pub fn message_log(&self) -> &MessageLog {
        &self.message_log
    }

    pub fn config(&self) -> &AgentConfig {
        &self.config
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

    /// Stream agent execution on a task (no tools).
    /// Returns a stream of tokens as they arrive.
    pub fn stream_run(
        &mut self,
        task: &str,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamToken, AgentError>> + Send>> {
        self.message_log.add_user(task.to_string());
        self.stream_loop(None)
    }

    /// Run agent on a task with tools available, streaming results.
    pub fn run_streaming_with_tools(
        &mut self,
        task: &str,
        tools: Arc<ToolRegistry>,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamToken, AgentError>> + Send>> {
        self.message_log.add_user(task.to_string());
        self.stream_loop(Some(tools))
    }

    /// Save current agent state to session
    pub async fn save_state(
        &self,
        manager: &crate::session::SessionManager,
    ) -> Result<(), crate::session::SessionError> {
        let metadata =
            crate::session::serializer::SessionSerializer::new_state(self.config.name.clone())
                .metadata;
        let state = crate::session::AgentState {
            agent_id: self.config.name.clone(),
            message_log: self.message_log.messages.clone(),
            context: serde_json::json!({}),
            metadata,
        };

        manager.update(&self.config.name, state).await
    }

    /// Restore agent state from session
    pub async fn restore_state(
        &mut self,
        manager: &crate::session::SessionManager,
    ) -> Result<(), crate::session::SessionError> {
        let state = manager.get(&self.config.name).await?;
        self.message_log.messages = state.message_log;
        Ok(())
    }

    /// Auto-save after each turn (optional)
    pub async fn auto_save(&self, manager: &crate::session::SessionManager) {
        if let Err(e) = self.save_state(manager).await {
            eprintln!("Failed to auto-save session: {}", e);
        }
    }

    /// Internal streaming loop - handles token streaming with optional tools.
    fn stream_loop(
        &mut self,
        tools: Option<Arc<ToolRegistry>>,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamToken, AgentError>> + Send>> {
        let (tx, rx) = tokio::sync::mpsc::channel(32);

        let config = self.config.clone();
        let messages = self.message_log.clone();
        let llm = self.llm.clone();
        let tools_for_request = tools.as_ref().map(|t| t.to_openai_format_shared());

        tokio::spawn(async move {
            let mut messages = messages;

            let mut request_messages = Vec::with_capacity(messages.len() + 1);
            request_messages.push(OpenAiMessage::System {
                content: config.system_prompt.clone(),
            });
            messages.extend_api_format(&mut request_messages);

            let request = LlmRequest {
                model: config.model.clone(),
                messages: request_messages,
                tools: tools_for_request.clone(),
                temperature: config.temperature,
                max_tokens: config.max_tokens,
            };

            let stream = llm.stream_complete(request);

            // Pin the stream to use it in a loop
            let mut stream = Box::pin(stream);

            while let Some(token_result) = stream.next().await {
                match token_result {
                    Ok(token) => {
                        // Track message accumulation
                        match &token {
                            StreamToken::Text(s) => {
                                messages.append_assistant_chunk(s);
                            }
                            StreamToken::ToolCall { name, args } => {
                                if let Some(ref registry) = tools {
                                    let result = registry.execute(name, args).await;
                                    match result {
                                        Ok(res) => {
                                            messages.add_tool_result(
                                                name.clone(),
                                                format_tool_result_content(res),
                                            );
                                        }
                                        Err(e) => {
                                            let _ = tx.send(Err(AgentError::Tool(e))).await;
                                        }
                                    }
                                }
                            }
                            StreamToken::Done => {}
                        }

                        if tx.send(Ok(token)).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(AgentError::Session(e.to_string()))).await;
                        break;
                    }
                }
            }
        });

        Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))
    }

    async fn run_loop(
        &mut self,
        tools: Option<&ToolRegistry>,
    ) -> Result<AgentOutput, AgentError> {
        const MAX_ITERATIONS: usize = 10;
        let mut accumulated_text = String::new();
        let tools_for_request = tools.map(ToolRegistry::to_openai_format_shared);

        for _ in 0..MAX_ITERATIONS {
            let mut messages = Vec::with_capacity(self.message_log.len() + 1);
            messages.push(OpenAiMessage::System {
                content: self.config.system_prompt.clone(),
            });
            self.message_log.extend_api_format(&mut messages);

            let request = LlmRequest {
                model: self.config.model.clone(),
                messages,
                tools: tools_for_request.clone(),
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
                        let result = registry.execute(&name, &args).await?;
                        self.message_log
                            .add_tool_result(name, format_tool_result_content(result));
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
    use serde_json::json;

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

    #[test]
    fn test_append_assistant_chunk_coalesces_stream_tokens() {
        let mut log = MessageLog::new();
        log.append_assistant_chunk("Hel");
        log.append_assistant_chunk("lo");
        log.add_user("next".to_string());
        log.append_assistant_chunk("World");

        assert_eq!(log.len(), 3);
        let api = log.to_api_format();
        assert!(matches!(
            &api[0],
            OpenAiMessage::Assistant { content } if content == "Hello"
        ));
        assert!(matches!(
            &api[2],
            OpenAiMessage::Assistant { content } if content == "World"
        ));
    }

    #[test]
    fn test_format_tool_result_content_preserves_plain_strings() {
        assert_eq!(
            format_tool_result_content(serde_json::Value::String("done".to_string())),
            "done"
        );
    }

    #[test]
    fn test_format_tool_result_content_serializes_structured_json() {
        assert_eq!(
            format_tool_result_content(json!({"value": 42})),
            r#"{"value":42}"#
        );
    }
}
