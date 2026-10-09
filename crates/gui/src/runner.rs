//! Async bridge: streams agent responses on a Tokio runtime and forwards
//! events to the Tauri frontend through an [`async_channel`].

use std::collections::HashMap;
use std::sync::Arc;

use agent::{Agent, ContextBudget, StreamToken};
use futures::StreamExt;
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

use crate::session::{ChatMessage, Role, ToolEvent};

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
                Role::User if !message.text.is_empty() => session.add_user(message.text.clone()),
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
        prompt: String,
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
