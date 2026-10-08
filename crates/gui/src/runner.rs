//! Async bridge: streams agent responses on a Tokio runtime and forwards
//! events to the Tauri frontend through an [`async_channel`].

use std::sync::Arc;

use agent::{Agent, StreamToken};
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
    /// Token usage reported for the turn.
    Usage {
        /// Prompt (input) tokens.
        prompt: u64,
        /// Completion (output) tokens.
        completion: u64,
    },
    /// The turn finished normally.
    Done,
    /// The turn failed.
    Error(String),
}

/// Owns the Tokio runtime and the currently running stream task.
pub(crate) struct Runner {
    rt: Runtime,
    task: Option<JoinHandle<()>>,
}

impl Runner {
    /// Create a runner with its own multi-thread Tokio runtime.
    pub(crate) fn new() -> Self {
        Self {
            rt: Runtime::new().expect("tokio runtime"),
            task: None,
        }
    }

    /// Abort the in-flight stream, if any.
    ///
    /// Dropping the stream future closes the HTTP connection to the provider,
    /// which is the effective "stop generation" for the BOS agent.
    pub(crate) fn stop(&mut self) {
        if let Some(task) = self.task.take() {
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

    /// Start streaming `prompt` with `agent`, returning the event receiver.
    ///
    /// Any previous task is aborted first; there is a single streaming slot.
    pub(crate) fn spawn(
        &mut self,
        agent: Arc<Agent>,
        prompt: String,
        transcript: Vec<ChatMessage>,
    ) -> async_channel::Receiver<AgentEvent> {
        self.stop();
        Self::seed_session(&agent, &transcript);

        let (tx, rx) = async_channel::unbounded();
        self.task = Some(self.rt.spawn(async move {
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
                        })
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
        }));
        rx
    }
}
