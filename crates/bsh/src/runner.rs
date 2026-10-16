//! Streaming bridge: runs the agent on a Tokio runtime and forwards events.
//!
//! The shell never awaits a model. A turn is a task on the runner's own Tokio
//! runtime that pushes AgentEvents into an async channel the shell polls from
//! the GPUI foreground executor.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use agent::{Agent, ContextBudget, StreamToken};
use futures::StreamExt;
use react::llm::Content;
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

use crate::model::{ChatMessage, Role};

/// One event the runner forwards to the shell.
#[derive(Debug, Clone)]
pub(crate) enum AgentEvent {
    /// More visible assistant text.
    Text(String),
    /// More reasoning text.
    Reasoning(String),
    /// The model called a tool.
    ToolCall {
        /// Tool name.
        name: String,
        /// Serialized arguments.
        args: String,
    },
    /// A tool returned.
    ToolResult {
        /// Tool name.
        name: String,
        /// Tool output.
        output: String,
        /// Milliseconds spent.
        ms: u64,
    },
    /// Provider-reported token usage for the turn.
    Usage {
        /// Prompt tokens.
        prompt: u64,
        /// Completion tokens.
        completion: u64,
        /// Prompt tokens the provider served from its cache.
        cached: u64,
    },
    /// Send-side compaction trimmed the outgoing history.
    Compacted {
        /// Tokens before.
        before: usize,
        /// Tokens after.
        after: usize,
        /// Messages dropped.
        dropped: usize,
    },
    /// The turn finished.
    Done,
    /// The turn failed.
    Error(String),
}

/// Owns the Tokio runtime and the in-flight turns.
pub(crate) struct Runner {
    rt: Runtime,
    tasks: HashMap<String, JoinHandle<()>>,
    stops: HashMap<String, Arc<AtomicBool>>,
}

impl Runner {
    /// Create a runner with its own multi-thread runtime.
    pub(crate) fn new() -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("bsh builds its tokio runtime");
        Self {
            rt,
            tasks: HashMap::new(),
            stops: HashMap::new(),
        }
    }

    /// Abort the in-flight stream for a session, if any.
    pub(crate) fn stop(&mut self, session_id: &str) {
        if let Some(flag) = self.stops.remove(session_id) {
            flag.store(true, Ordering::SeqCst);
        }
        if let Some(task) = self.tasks.remove(session_id) {
            task.abort();
        }
    }

    /// Re-seed the agent session from the UI transcript.
    pub(crate) fn seed_session(agent: &Agent, transcript: &[ChatMessage]) {
        let mut session = agent.session();
        session.restore_messages(Vec::new());
        for message in transcript {
            match message.role {
                Role::User if !message.text.is_empty() => {
                    session.add_user_content(Content::Text(message.text.clone()));
                }
                Role::Assistant if !message.text.is_empty() => {
                    session.add_assistant(message.text.clone());
                }
                _ => {}
            }
        }
    }

    /// Start streaming a turn, returning the event receiver.
    /// The tokio runtime bsh drives blocking-capable I/O on.
    ///
    /// The gpui executor has no reactor, so a reqwest call started from a
    /// view has to be handed to this runtime and posted back.
    pub(crate) fn rt(&self) -> &Runtime {
        &self.rt
    }
    pub(crate) fn spawn(
        &mut self,
        session_id: &str,
        agent: Arc<Agent>,
        input: Content,
        transcript: Vec<ChatMessage>,
        context_budget: usize,
        tool_output_cap: usize,
    ) -> async_channel::Receiver<AgentEvent> {
        self.stop(session_id);
        Self::seed_session(&agent, &transcript);

        let (tx, rx) = async_channel::unbounded();
        if context_budget > 0 {
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
        }

        let stop = Arc::new(AtomicBool::new(false));
        let stop_task = stop.clone();
        let handle = self.rt.spawn(async move {
            let mut stream = agent.stream(input);
            while let Some(item) = stream.next().await {
                if stop_task.load(Ordering::SeqCst) {
                    break;
                }
                let terminal =
                    matches!(&item, Ok(StreamToken::Done | StreamToken::Stopped) | Err(_));
                let event = match item {
                    Ok(StreamToken::Text(text)) => AgentEvent::Text(text),
                    Ok(StreamToken::ReasoningContent(text)) => AgentEvent::Reasoning(text),
                    Ok(StreamToken::ToolCall { name, args, .. }) => AgentEvent::ToolCall {
                        name,
                        args: args.to_string(),
                    },
                    Ok(StreamToken::ToolResult { name, output, ms }) => {
                        let output = if output.chars().count() > tool_output_cap {
                            let cut: String = output.chars().take(tool_output_cap).collect();
                            format!("{cut}\n... [output truncated for display]")
                        } else {
                            output
                        };
                        AgentEvent::ToolResult { name, output, ms }
                    }
                    Ok(StreamToken::Usage(usage)) => AgentEvent::Usage {
                        prompt: u64::from(usage.prompt_tokens),
                        completion: u64::from(usage.completion_tokens),
                        // The provider's cache breakdown rides on the raw
                        // provider usage, the same way the engine reads it.
                        cached: u64::from(
                            usage
                                .prompt_tokens_details
                                .as_ref()
                                .and_then(|details| details.cached_tokens)
                                .unwrap_or(0),
                        ),
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
        self.tasks.retain(|_, task| !task.is_finished());
        self.tasks.insert(session_id.to_string(), handle);
        self.stops.insert(session_id.to_string(), stop);
        rx
    }
}
