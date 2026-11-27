//! Replaying a stored transcript into an agent session.
//!
//! A turn reads the conversation before it, and both interfaces must replay the
//! SAME rows: text, reasoning where the provider needs it, tool calls with their
//! stored results, and a user row's images. The window's runner and the Host's
//! executor therefore share this module; a second replay would let the two
//! surfaces ask the model different questions about one conversation.

use agent::Agent;

use crate::model::{ChatMessage, Role};

/// The multimodal content one stored user row replays as.
///
/// A row with images replays as the same block its live turn sent, which is how
/// a goal attachment message stays readable by a later round. Parsing is done
/// here, once, so a second caller cannot disagree about a data URL.
pub(crate) fn message_content(message: &ChatMessage) -> Option<react::llm::Content> {
    if message.images.is_empty() {
        return (!message.text.is_empty()).then(|| react::llm::Content::Text(message.text.clone()));
    }
    let mut parts: Vec<react::llm::ContentPart> = Vec::new();
    if !message.text.is_empty() {
        parts.push(react::llm::ContentPart::Text {
            text: message.text.clone(),
        });
    }
    for url in &message.images {
        let Some((mime, base64)) = parse_data_url(url) else {
            continue;
        };
        if let react::llm::Content::Parts(mut block) =
            react::llm::Content::binary(mime.to_string(), base64.to_string())
        {
            parts.append(&mut block);
        }
    }
    (!parts.is_empty()).then_some(react::llm::Content::Parts(parts))
}

/// Split a data URL into its MIME type and base64 payload.
pub(crate) fn parse_data_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("data:")?;
    let (mime, payload) = rest.split_once(";base64,")?;
    (!mime.is_empty() && !payload.is_empty()).then_some((mime, payload))
}

/// Re-seed the agent session from the stored transcript.
///
/// The stored history replays in the model's own order: text, then each tool
/// call with its result. The runtime snapshot and workspace instructions are
/// injected by the caller after compaction, immediately before the stream, so
/// the fresh context is never folded into the checkpoint it was meant to
/// supersede.
pub(crate) fn seed_session(agent: &Agent, transcript: &[ChatMessage]) {
    let commands = crate::extension::cached_commands();
    seed_session_with(agent, transcript, &commands);
}

/// The same seed against an explicit command list.
///
/// Split out so the replay of a `/command` annotation is testable without a
/// live extension host: the production path is [seed_session], which reads the
/// host's current list once per seed.
pub(crate) fn seed_session_with(
    agent: &Agent,
    transcript: &[ChatMessage],
    commands: &[crate::extension::CommandSurface],
) {
    let mut session = agent.session();
    session.restore_messages(Vec::new());
    for (row_index, message) in transcript.iter().enumerate() {
        match message.role {
            Role::User => {
                if let Some(content) = message_content(message) {
                    session.add_user_content(crate::command_invocation::replay(commands, content));
                }
            }
            Role::Assistant => {
                // Reasoning is replayed only to a provider that requires it
                // (DeepSeek rejects a tool-carrying request whose earlier turns
                // lost their chain of thought). Every other provider keeps the
                // turn-local rule: the transcript still shows the block to the
                // reader, but the request must not carry it.
                if agent.replays_reasoning_across_turns() && !message.reasoning.trim().is_empty() {
                    session.push(react::llm::LlmMessage::AssistantReasoning {
                        content: message.reasoning.clone(),
                        provider_state: None,
                    });
                }
                if !message.text.is_empty() {
                    session.add_assistant(message.text.clone());
                }
                // The tool traffic replays too. Dropping it used to make every
                // turn re-do the last turn's reads - the model re-read the same
                // files each turn because the results it already held were gone
                // - and it moved the request prefix, so the provider's prompt
                // cache never carried across turns. The stored outputs are the
                // capped ones the request sent; the ids are seed-local and only
                // need to pair call with result inside this history.
                for (call_index, event) in message.tools.iter().enumerate() {
                    let call_id = format!("seed-{row_index}-{call_index}");
                    let args: serde_json::Value =
                        serde_json::from_str(&event.args).unwrap_or(serde_json::json!({}));
                    session.push(react::llm::LlmMessage::AssistantToolCall {
                        tool_call_id: call_id.clone(),
                        name: event.name.clone(),
                        args,
                    });
                    session.push(react::llm::LlmMessage::ToolResult {
                        tool_call_id: call_id,
                        content: event.output.clone().unwrap_or_else(|| {
                            "The tool call produced no recorded result.".to_string()
                        }),
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::model::ChatMessage;

    /// A user row with an image replays as the same multimodal block it was sent
    /// as; the data URL is parsed once here rather than by each caller.
    #[test]
    fn an_image_row_replays_as_its_multimodal_block() {
        let mut row = ChatMessage::user("look");
        row.images.push("data:image/png;base64,Zg==".to_string());
        let content = super::message_content(&row).expect("multimodal content");
        match content {
            react::llm::Content::Parts(parts) => {
                assert!(
                    matches!(&parts[0], react::llm::ContentPart::Text { text } if text == "look")
                );
                assert!(matches!(&parts[1], react::llm::ContentPart::Binary { .. }));
            }
            other => panic!("expected a multimodal block, got {other:?}"),
        }
    }

    #[test]
    fn a_data_url_needs_a_mime_and_a_payload() {
        assert_eq!(
            super::parse_data_url("data:image/png;base64,Zg=="),
            Some(("image/png", "Zg=="))
        );
        assert_eq!(super::parse_data_url("data:;base64,Zg=="), None);
        assert_eq!(super::parse_data_url("http://example.com/a.png"), None);
    }
}
