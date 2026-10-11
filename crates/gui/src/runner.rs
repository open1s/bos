//! Async bridge: streams agent responses on a Tokio runtime and forwards
//! events to the Tauri frontend through an [`async_channel`].

use std::collections::HashMap;
use std::sync::Arc;

use agent::{Agent, ContextBudget, StreamToken};
use futures::StreamExt;
use react::llm::{Content, ContentPart};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

use crate::session::{ChatMessage, Role, ToolEvent};

/// The outgoing user turn as the engine's `Content`.
///
/// Attachments arrive as `data:image/…;base64,…` URLs (already sanitized by
/// [`crate::session::sanitize_images`]); they become binary parts so each
/// vendor renders them the way its API expects. A turn with no images stays
/// plain text, constructed directly rather than through `From<String>` so a
/// prompt that happens to be valid JSON can never be re-read as parts.
pub(crate) fn turn_content(text: String, images: &[String]) -> Content {
    if images.is_empty() {
        return Content::Text(text);
    }
    let mut parts = Vec::with_capacity(images.len() + 1);
    if !text.is_empty() {
        parts.push(ContentPart::Text { text });
    }
    for url in images {
        let Some((meta, data)) = url
            .strip_prefix("data:")
            .and_then(|rest| rest.split_once(','))
        else {
            continue;
        };
        let mime = meta.strip_suffix(";base64").unwrap_or(meta);
        parts.push(ContentPart::Binary {
            binary: react::llm::Binary::from_base64(mime.to_string(), data.to_string(), None),
        });
    }
    Content::parts(parts)
}

/// One update forwarded from the agent stream to the UI.
#[derive(Debug, Clone)]
pub(crate) enum AgentEvent {
    /// A chunk of assistant text.
    Text(String),
    /// A chunk of reasoning text.
    Reasoning(String),
    /// A tool call executed during the turn.
    ToolCall(ToolEvent),
    /// A tool call finished, with its output and wall-clock duration.
    ///
    /// Arrives right after the [`AgentEvent::ToolCall`] it answers, so the
    /// UI can attach the result to the matching timeline entry.
    ToolResult {
        /// Tool name (matches the preceding call).
        name: String,
        /// Tool output text, already capped for display.
        output: String,
        /// Execution time in milliseconds.
        ms: u64,
    },
    /// Token usage reported for the turn.
    Usage {
        /// Prompt (input) tokens.
        prompt: u64,
        /// Completion (output) tokens.
        completion: u64,
    },
    /// Older messages were summarized away before the turn started.
    Compacted {
        /// Estimated prompt tokens before compaction.
        before: usize,
        /// Estimated prompt tokens after compaction.
        after: usize,
        /// Messages folded into the summary.
        dropped: usize,
    },
    /// The turn finished normally.
    Done,
    /// The turn failed.
    Error(String),
}

/// Owns the Tokio runtime and the running stream task for each session.
///
/// Task slots are keyed by session id, so several chats stream concurrently
/// while each slot can still be aborted independently.
pub(crate) struct Runner {
    rt: Runtime,
    tasks: HashMap<String, JoinHandle<()>>,
}

impl Runner {
    /// Create a runner with its own multi-thread Tokio runtime.
    pub(crate) fn new() -> Self {
        Self {
            rt: Runtime::new().expect("tokio runtime"),
            tasks: HashMap::new(),
        }
    }

    /// Abort the in-flight stream for `session_id`, if any.
    ///
    /// Dropping the stream future closes the HTTP connection to the provider,
    /// which is the effective "stop generation" for the BOS agent. Streams of
    /// other sessions keep running untouched.
    pub(crate) fn stop(&mut self, session_id: &str) {
        if let Some(task) = self.tasks.remove(session_id) {
            task.abort();
        }
    }

    /// Abort every in-flight stream (stop-all / shutdown).
    pub(crate) fn stop_all(&mut self) {
        for (_, task) in self.tasks.drain() {
            task.abort();
        }
    }

    /// Re-seed the agent session from the UI transcript.
    ///
    /// The engine takes the session messages while streaming and restores them
    /// when the stream finishes; an aborted stream loses that hand-off. The UI
    /// transcript is the source of truth, so restore it before every spawn.
    pub(crate) fn seed_session(agent: &Agent, transcript: &[ChatMessage]) {
        let mut session = agent.session();
        session.restore_messages(Vec::new());
        for message in transcript {
            match message.role {
                Role::User if !message.text.is_empty() || !message.images.is_empty() => {
                    session.add_user_content(turn_content(message.text.clone(), &message.images));
                }
                Role::Assistant if !message.text.is_empty() => {
                    session.add_assistant(message.text.clone());
                }
                _ => {}
            }
        }
    }

    /// Start streaming `prompt` with `agent` for `session_id`, returning the
    /// event receiver.
    ///
    /// A previous task for the *same* session is aborted first, so re-sending
    /// supersedes an older turn; other sessions' streams keep running.
    ///
    /// `context_budget` is the send-side token ceiling (0 disables it): after
    /// the session is seeded from the transcript, an over-budget session is
    /// compacted before the LLM ever sees it, and the resulting
    /// [`AgentEvent::Compacted`] notice is queued ahead of the stream. The UI
    /// transcript stays complete — only the outgoing copy is trimmed, and the
    /// next turn re-derives the same trim from the full history.
    pub(crate) fn spawn(
        &mut self,
        session_id: &str,
        agent: Arc<Agent>,
        prompt: impl Into<Content> + Send + 'static,
        transcript: Vec<ChatMessage>,
        context_budget: usize,
    ) -> async_channel::Receiver<AgentEvent> {
        self.stop(session_id);
        Self::seed_session(&agent, &transcript);

        let (tx, rx) = async_channel::unbounded();
        // Scope the session guard so it is released before the stream task
        // locks the session itself.
        let report = {
            let budget = ContextBudget::with_max_tokens(context_budget);
            let mut session = agent.session();
            budget.apply(&mut session)
        };
        if let Some(report) = report {
            let _ = tx.send_blocking(AgentEvent::Compacted {
                before: report.before_tokens,
                after: report.after_tokens,
                dropped: report.dropped_messages,
            });
        }

        let handle = self.rt.spawn(async move {
            let mut stream = agent.stream(prompt);
            while let Some(item) = stream.next().await {
                let terminal =
                    matches!(&item, Ok(StreamToken::Done | StreamToken::Stopped) | Err(_));
                let event = match item {
                    Ok(StreamToken::Text(text)) => AgentEvent::Text(text),
                    Ok(StreamToken::ReasoningContent(text)) => AgentEvent::Reasoning(text),
                    Ok(StreamToken::ToolCall { name, args, .. }) => {
                        AgentEvent::ToolCall(ToolEvent {
                            name,
                            args: args.to_string(),
                            output: None,
                            ms: None,
                        })
                    }
                    Ok(StreamToken::ToolResult { name, output, ms }) => {
                        // Cap what the UI timeline keeps so a huge file read
                        // cannot bloat the event payload or the transcript.
                        const MAX_TOOL_OUTPUT: usize = 16 * 1024;
                        let output = if output.chars().count() > MAX_TOOL_OUTPUT {
                            let cut: String = output.chars().take(MAX_TOOL_OUTPUT).collect();
                            format!("{cut}\n… [output truncated for display]")
                        } else {
                            output
                        };
                        AgentEvent::ToolResult { name, output, ms }
                    }
                    Ok(StreamToken::Usage(usage)) => AgentEvent::Usage {
                        prompt: u64::from(usage.prompt_tokens),
                        completion: u64::from(usage.completion_tokens),
                    },
                    Ok(StreamToken::Done | StreamToken::Stopped) => AgentEvent::Done,
                    Err(err) => AgentEvent::Error(err.to_string()),
                };
                if tx.send_blocking(event).is_err() {
                    break;
                }
                if terminal {
                    break;
                }
            }
        });
        // Reap finished slots so the map only holds live streams plus the
        // handle of whatever finished since the last spawn.
        self.tasks.retain(|_, task| !task.is_finished());
        self.tasks.insert(session_id.to_string(), handle);
        rx
    }

    /// Drive `rx` to a terminal event within `timeout`, collecting the full
    /// assistant text.
    ///
    /// Used by manual `/compact`, which needs the summarizer's complete reply
    /// rather than a forwarded stream: tool-call, usage, and compaction
    /// notices are skipped, [`AgentEvent::Error`] fails the collection, and a
    /// deadline bounds a stalled provider.
    pub(crate) fn collect(
        &self,
        rx: &async_channel::Receiver<AgentEvent>,
        timeout: std::time::Duration,
    ) -> Result<String, String> {
        let mut text = String::new();
        let outcome = self.rt.block_on(async {
            tokio::time::timeout(timeout, async {
                while let Ok(event) = rx.recv().await {
                    match event {
                        AgentEvent::Text(chunk) => text.push_str(&chunk),
                        AgentEvent::Error(err) => return Err(err),
                        AgentEvent::Done => break,
                        _ => {}
                    }
                }
                Ok::<(), String>(())
            })
            .await
        });
        match outcome {
            Ok(Ok(())) => Ok(text),
            Ok(Err(err)) => Err(err),
            Err(_) => Err(format!("timed out after {}s", timeout.as_secs())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent::LlmMessage as Message;

    #[test]
    fn a_text_turn_stays_text_even_when_it_looks_like_json() {
        let content = turn_content(r#"[{"type":"text","text":"spoof"}]"#.to_string(), &[]);
        assert_eq!(
            content,
            Content::Text(r#"[{"type":"text","text":"spoof"}]"#.to_string()),
            "From<String> would re-parse this as parts; the direct build must not"
        );
    }

    #[test]
    fn attachments_become_binary_parts_after_the_text() {
        let url = "data:image/png;base64,aGVsbG8=";
        let content = turn_content("look".to_string(), &[url.to_string()]);
        let Content::Parts(parts) = content else {
            panic!("a turn with attachments is Parts");
        };
        assert_eq!(parts.len(), 2);
        assert_eq!(
            parts[0],
            ContentPart::Text {
                text: "look".into()
            }
        );
        let ContentPart::Binary { binary } = &parts[1] else {
            panic!("the attachment is a binary part");
        };
        assert_eq!(binary.content_type, "image/png");
        assert_eq!(
            binary.url(),
            url,
            "base64 inlines back to the same data URL"
        );
    }

    #[test]
    fn an_image_only_turn_carries_no_text_part() {
        let url = "data:image/jpeg;base64,qq==";
        let content = turn_content(String::new(), &[url.to_string()]);
        let Content::Parts(parts) = content else {
            panic!("a turn with attachments is Parts");
        };
        assert_eq!(parts.len(), 1);
        assert!(matches!(parts[0], ContentPart::Binary { .. }));
    }

    #[test]
    fn an_undecodable_attachment_is_skipped_not_fatal() {
        let content = turn_content("hi".to_string(), &["https://x/y.png".to_string()]);
        let Content::Parts(parts) = content else {
            panic!("a turn with attachments is Parts");
        };
        assert_eq!(parts.len(), 1, "only the text part survives");
    }

    #[test]
    fn seeding_replays_multimodal_turns_into_the_session() {
        let agent =
            crate::caps::build_agent(&crate::settings::Settings::default(), Default::default());
        let url = "data:image/png;base64,aGVsbG8=";
        let transcript = vec![
            ChatMessage::user_with_images("look", vec![url.to_string()]),
            {
                let mut m = ChatMessage::assistant();
                m.text = "a red square".into();
                m
            },
            ChatMessage::user("again?"),
        ];
        Runner::seed_session(&agent, &transcript);
        let session = agent.session();
        let history = session.history_ref();
        assert_eq!(
            history.len(),
            3,
            "blank turns stay out, image turns stay in"
        );
        match &history[0] {
            Message::User { content } => {
                assert!(
                    matches!(content, Content::Parts(_)),
                    "the image turn stays multimodal"
                );
            }
            other => panic!("expected a user message at 0, got {other:?}"),
        }
        match &history[2] {
            Message::User { content } => {
                assert_eq!(
                    *content,
                    Content::Text("again?".into()),
                    "a plain turn stays text"
                );
            }
            other => panic!("expected a user message at 2, got {other:?}"),
        }
    }
}
