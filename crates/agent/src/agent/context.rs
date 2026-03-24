use crate::{OpenAiMessage};
use crate::agent::message::Message;

#[derive(Debug, Clone, Default)]
pub struct MessageContext {
    pub(crate) messages: Vec<Message>,
}

impl MessageContext {
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
