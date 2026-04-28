use crate::OpenAiMessage;
use react::llm::vendor::ToolCall;
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

    pub fn add_tool_call(&mut self, tool_call_id: String, name: String, args: serde_json::Value) {
        self.messages.push(Message::AssistantToolCall {
            tool_call_id,
            name,
            args,
        });
    }

    pub fn add_tool_result(&mut self, name: String, content: String) {
        self.messages.push(Message::ToolResult {
            tool_call_id: name,
            content,
        });
    }

    pub fn to_api_format(&self) -> Vec<OpenAiMessage> {
        let mut api_messages = Vec::with_capacity(self.messages.len());
        self.extend_api_format(&mut api_messages);
        api_messages
    }

    pub fn extend_api_format(&self, target: &mut Vec<OpenAiMessage>) {
        target.reserve(self.messages.len());
        for message in &self.messages {
            match message {
                Message::User { content } => {
                    target.push(OpenAiMessage {
                        role: "user".to_string(),
                        content: Some(content.clone()),
                        tool_calls: None,
                        function_call: None,
                        reasoning_content: None,
                        extra: serde_json::Value::Object(serde_json::Map::new()),
                    });
                }
                Message::System { content } => {
                    target.push(OpenAiMessage {
                        role: "system".to_string(),
                        content: Some(content.clone()),
                        tool_calls: None,
                        function_call: None,
                        reasoning_content: None,
                        extra: serde_json::Value::Object(serde_json::Map::new()),
                    });
                }
                Message::Assistant { content } => {
                    target.push(OpenAiMessage {
                        role: "assistant".to_string(),
                        content: Some(content.clone()),
                        tool_calls: None,
                        function_call: None,
                        reasoning_content: None,
                        extra: serde_json::Value::Object(serde_json::Map::new()),
                    });
                }
                Message::AssistantToolCall {
                    tool_call_id,
                    name,
                    args,
                } => {
                    target.push(OpenAiMessage {
                        role: "assistant".to_string(),
                        content: None,
                        tool_calls: Some(vec![ToolCall {
                            id: tool_call_id.clone(),
                            function: react::llm::vendor::FunctionCall {
                                name: name.clone(),
                                arguments: args.to_string(),
                            },
                        }]),
                        function_call: None,
                        reasoning_content: None,
                        extra: serde_json::Value::Object(serde_json::Map::new()),
                    });
                }
                Message::ToolResult {
                    tool_call_id,
                    content,
                } => {
                    target.push(OpenAiMessage {
                        role: "tool".to_string(),
                        content: Some(content.clone()),
                        tool_calls: None,
                        function_call: None,
                        reasoning_content: None,
                        extra: serde_json::Value::Object(serde_json::Map::new()),
                    });
                }
            }
        }
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}