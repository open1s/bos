//! Byte accounting for the parts of one model request.
//!
//! A provider bills and the provider caches the WHOLE request, so "how big is
//! the prompt" is not one number: the system prompt, the tool definitions, the
//! skill catalogue, the conversation history and the new input each ride every
//! step. A capture that only shows the total cannot attribute a 100 KB request,
//! which is what this module is for.
//!
//! The measurement is taken before dispatch, from the same values the vendor
//! serializes, and counts bytes with a writer that discards them: nothing is
//! allocated for a history or a schema just to be priced. It is a composition
//! measurement, not an invoice: the wire envelope a provider adds (field names,
//! protocol framing) is not included, which is why the total is reported beside
//! the transport tap's own `request_bytes` instead of replacing it.
//!
//! Only counts leave this module. No message text, tool argument or credential
//! is recorded, so the report can be logged without a privacy decision.

use serde::{Deserialize, Serialize};

use crate::llm::{Content, LlmMessage, LlmTool, Skill};

/// The byte sizes that make up one request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestComposition {
    /// The system prompt this request carries.
    pub system_bytes: usize,
    /// Every advertised tool definition, name and schema.
    pub tools_bytes: usize,
    /// How many definitions the array carries.
    pub tool_count: usize,
    /// The largest single definition's bytes.
    pub largest_tool_bytes: usize,
    /// The name of that definition, when the array is not empty.
    pub largest_tool: Option<String>,
    /// The skill catalogue, when the host advertises one inline.
    pub skills_bytes: usize,
    /// The conversation history.
    pub history_bytes: usize,
    /// The new input for this step.
    pub input_bytes: usize,
    /// The sum of the parts above.
    pub total_bytes: usize,
}

impl RequestComposition {
    /// Price one request from the values the vendor will serialize.
    pub fn measure(
        persona: Option<&str>,
        tools: &[LlmTool],
        skills: Option<&[Skill]>,
        history: Option<&[LlmMessage]>,
        input: &Content,
    ) -> Self {
        let system_bytes = persona.map_or(0, str::len);
        let mut tools_bytes = 0usize;
        let mut largest_tool_bytes = 0usize;
        let mut largest_tool: Option<String> = None;
        for tool in tools {
            let bytes = tool.schema_bytes();
            tools_bytes = tools_bytes.saturating_add(bytes);
            if bytes > largest_tool_bytes {
                largest_tool_bytes = bytes;
                largest_tool = Some(tool.name.clone());
            }
        }
        let skills_bytes = skills.map_or(0, json_len);
        let history_bytes = history.map_or(0, json_len);
        let input_bytes = json_len(input);
        let total_bytes = system_bytes
            .saturating_add(tools_bytes)
            .saturating_add(skills_bytes)
            .saturating_add(history_bytes)
            .saturating_add(input_bytes);
        Self {
            system_bytes,
            tools_bytes,
            tool_count: tools.len(),
            largest_tool_bytes,
            largest_tool,
            skills_bytes,
            history_bytes,
            input_bytes,
            total_bytes,
        }
    }
}

/// A writer that keeps the count and drops the bytes.
struct ByteCounter(usize);

impl std::io::Write for ByteCounter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(buffer.len());
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The serialized byte length of one value, without keeping the serialization.
fn json_len<T: Serialize + ?Sized>(value: &T) -> usize {
    let mut counter = ByteCounter(0);
    // A value that cannot be serialized is not a request the vendor can send;
    // zero is the honest reading and it keeps the meter from panicking on the
    // request path.
    match serde_json::to_writer(&mut counter, value) {
        Ok(()) => counter.0,
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history() -> Vec<LlmMessage> {
        vec![
            LlmMessage::user_text("hello"),
            LlmMessage::assistant("hi there".to_string()),
        ]
    }

    #[test]
    fn the_parts_add_up_to_the_total() {
        let tools = vec![LlmTool::function(
            "bash",
            "run a command",
            serde_json::json!({"type": "object"}),
        )];
        let composition = RequestComposition::measure(
            Some("system prompt"),
            &tools,
            None,
            Some(&history()),
            &Content::text("next question"),
        );

        assert_eq!(composition.system_bytes, "system prompt".len());
        assert_eq!(composition.tool_count, 1);
        assert_eq!(composition.tools_bytes, tools[0].schema_bytes());
        assert!(composition.history_bytes > 0, "the transcript is priced");
        assert_eq!(
            composition.total_bytes,
            composition.system_bytes
                + composition.tools_bytes
                + composition.skills_bytes
                + composition.history_bytes
                + composition.input_bytes
        );
    }

    #[test]
    fn the_largest_definition_is_named() {
        let tools = vec![
            LlmTool::function("small", "s", serde_json::json!({})),
            LlmTool::function("large", "l".repeat(500), serde_json::json!({})),
        ];
        let composition = RequestComposition::measure(None, &tools, None, None, &Content::text(""));
        assert_eq!(composition.largest_tool.as_deref(), Some("large"));
        assert_eq!(composition.largest_tool_bytes, tools[1].schema_bytes());
    }

    #[test]
    fn an_empty_request_carries_only_its_input() {
        let composition = RequestComposition::measure(None, &[], None, None, &Content::text(""));
        assert_eq!(composition.total_bytes, composition.input_bytes);
        assert_eq!(composition.tool_count, 0);
        assert_eq!(composition.system_bytes, 0);
        assert_eq!(composition.tools_bytes, 0);
        assert!(composition.largest_tool.is_none());
    }

    #[test]
    fn a_binary_payload_is_priced_by_its_escaped_size() {
        let payload = "A".repeat(4_096);
        let content = Content::binary("image/png", payload);
        let composition = RequestComposition::measure(None, &[], None, None, &content);
        assert!(
            composition.input_bytes >= 4_096,
            "the payload bytes are counted: {}",
            composition.input_bytes
        );
    }
}
