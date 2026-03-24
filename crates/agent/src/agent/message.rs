use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Message {
    User(String),
    Assistant(String),
    ToolResult { name: String, content: String },
}