use crate::OpenAiMessage;
use react::Message;
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
        self.messages.push(Message::User { content });
    }

    pub fn add_system(&mut self, profile: String) {
        self.messages.push(Message::System { content: profile });
    }

    pub fn add_assistant(&mut self, content: String) {
        self.messages.push(Message::Assistant { content });
    }

    pub fn append_assistant_chunk(&mut self, chunk: &str) {
        match self.messages.last_mut() {
            Some(Message::Assistant { content }) => content.push_str(chunk),
            _ => self.messages.push(Message::Assistant {
                content: chunk.to_string(),
            }),
        }
    }

    pub fn add_tool_call(&mut self, id: String, name: String, args: serde_json::Value) {
        self.messages
            .push(Message::AssistantToolCall { id, name, args });
    }

    pub fn add_tool_result(&mut self, name: String, content: String) {
        self.messages.push(Message::ToolResult {
            tool_call_id: name,
            content,
        });
    }

    // Patch D placeholder: record a policy decision for a given tool.
    // Currently a no-op; wired later when real policy hooks are introduced.
    pub fn add_policy_decision(&mut self, _tool: &str, _allowed: bool, _reason: &str) {
        // no-op for now
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
                Message::User { content } => OpenAiMessage::User {
                    content: content.clone(),
                },
                Message::System { content: profile } => OpenAiMessage::System {
                    content: profile.clone(),
                },
                Message::Assistant { content } => OpenAiMessage::Assistant {
                    content: content.clone(),
                },
                Message::AssistantToolCall { id, name, args } => OpenAiMessage::AssistantToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    args: args.clone(),
                },
                Message::ToolResult {
                    tool_call_id,
                    content,
                } => OpenAiMessage::ToolResult {
                    tool_call_id: tool_call_id.clone(),
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
