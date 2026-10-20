//! Streaming bridge: runs the agent on a Tokio runtime and forwards events.
//!
//! The shell never awaits a model. A turn is a task on the runner's own Tokio
//! runtime that pushes AgentEvents into an async channel the shell polls from
//! the GPUI foreground executor.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use agent::{Agent, StreamToken};
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
    /// An automatic compaction condensed the over-threshold history.
    ///
    /// This lands before the turn's first token, the same point the reference's
    /// `agent/pre-step` pressure listener runs. `dropped` counts the messages
    /// the summarizer removed from the request window it was handed; the shell
    /// maps that span onto the stored transcript and draws the marker.
    AutoCompacted {
        /// Estimated tokens in the replaced span.
        before: usize,
        /// Messages the checkpoint replaced.
        dropped: usize,
        /// The model-written checkpoint text, unframed.
        summary: String,
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

    /// Run a detached child task on the same runtime the turns use.
    ///
    /// GPUI's executor is not a Tokio runtime, so a child turn that awaits a
    /// Tokio timer (the engine's LLM timeout) must be spawned here.
    pub(crate) fn spawn_task<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.rt.spawn(future)
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
                // A stored user row with images replays as the same multimodal
                // content its live turn sent, which is how the /goal attachment
                // message stays readable by a later goal round.
                Role::User => {
                    if let Some(content) = crate::state::message_content(message) {
                        session.add_user_content(content);
                    }
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
        auto_retain: Option<usize>,
        tool_output_cap: usize,
    ) -> async_channel::Receiver<AgentEvent> {
        self.stop(session_id);
        Self::seed_session(&agent, &transcript);

        let (tx, rx) = async_channel::unbounded();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_task = stop.clone();
        let handle = self.rt.spawn(async move {
            // The reference's automatic trigger is an `agent/pre-step` listener
            // that condenses the oldest balanced span before the request is
            // derived. bsh runs that auxiliary request here, after the window is
            // seeded and before the turn streams, so the first request already
            // carries the checkpoint. A failure or a no-op continues on the
            // un-compacted history, exactly the reference's "log a warning and
            // continue the turn" rule.
            if let Some(retain) = auto_retain {
                if let Ok(Some(outcome)) = agent.compact_now(retain, &stop_task).await {
                    let event = AgentEvent::AutoCompacted {
                        before: outcome.before_tokens,
                        dropped: outcome.replaced,
                        summary: outcome.summary,
                    };
                    if tx.send_blocking(event).is_err() {
                        return;
                    }
                }
            }
            if stop_task.load(Ordering::SeqCst) {
                return;
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A child turn awaits the engine's Tokio timeout; it must run on the
    /// runner's runtime rather than GPUI's non-Tokio executor.
    #[test]
    fn a_child_task_gets_the_tokio_runtime() {
        let runner = Runner::new();
        let handle = runner.spawn_task(async {
            tokio::time::timeout(Duration::from_millis(5), async {})
                .await
                .is_ok()
        });
        let result = futures::executor::block_on(handle).expect("the child task ran");
        assert!(result, "the timeout completed");
    }

    /// A local vendor the automatic-compaction test can drive: the completion
    /// call answers with a checkpoint body, the stream answers with visible
    /// chunks and a Done.
    struct SummaryMock;

    fn summary_response(text: &str) -> react::llm::LlmResponse {
        use react::llm::vendor::{ChatCompletionResponse, ChatMessage, Choice, Usage};
        react::llm::LlmResponse::OpenAI(ChatCompletionResponse {
            id: "summary-mock".to_string(),
            object: "chat.completion".to_string(),
            created: 1,
            model: "mock-model".to_string(),
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: Some(text.to_string()),
                    tool_calls: None,
                    function_call: None,
                    reasoning_content: None,
                    extra: serde_json::Value::Object(serde_json::Map::new()),
                },
                finish_reason: Some("stop".to_string()),
                stop_reason: None,
                logprobs: None,
            }],
            usage: Some(Usage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
                prompt_tokens_details: None,
            }),
            system_fingerprint: None,
            nvext: None,
        })
    }

    #[async_trait::async_trait]
    impl agent::LlmClient<agent::agent::AgentSession, agent::agent::AgentReactContext> for SummaryMock {
        async fn complete(
            &self,
            _persona: Option<String>,
            _req: react::llm::LlmRequest,
            _session: &mut agent::agent::AgentSession,
            _context: &mut agent::agent::AgentReactContext,
        ) -> react::llm::LlmResponseResult {
            Ok(summary_response("## Current Work\n- auto checkpoint"))
        }

        async fn stream_complete(
            &self,
            _persona: Option<String>,
            _req: react::llm::LlmRequest,
            _session: &mut agent::agent::AgentSession,
            _context: &mut agent::agent::AgentReactContext,
        ) -> Result<react::llm::TokenStream, react::llm::LlmError> {
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(react::llm::StreamToken::Text("chunk1".to_string())),
                Ok(react::llm::StreamToken::Done),
            ])))
        }

        fn provider_name(&self) -> &'static str {
            "summary-mock"
        }
    }

    /// The automatic trigger runs BEFORE the turn streams: the summarizer
    /// installs its checkpoint on the agent, the runner reports AutoCompacted,
    /// and the streamed reply follows. This is the seam `spawn_turn` arms when
    /// the priced request reaches the workspace ceiling.
    #[test]
    fn an_over_threshold_turn_compacts_automatically_before_streaming() {
        use crate::model::ChatMessage;
        let mut provider = agent::agent::agentic::LlmProvider::new();
        provider.register_vendor("mock".to_string(), Box::new(SummaryMock));
        let agent = std::sync::Arc::new(agent::agent::Agent::new(
            agent::agent::AgentConfig {
                model: "mock-model".to_string(),
                ..Default::default()
            },
            std::sync::Arc::new(provider),
        ));
        let long = "detail ".repeat(60);
        let transcript = vec![
            ChatMessage::user(format!("one {long}")),
            ChatMessage::assistant(),
            ChatMessage::user(format!("two {long}")),
            ChatMessage::assistant(),
            ChatMessage::user(format!("three {long}")),
            ChatMessage::assistant(),
        ];
        let mut transcript = transcript;
        for (index, marker) in [(1, "r1"), (3, "r2"), (5, "r3")] {
            transcript[index].text = format!("{marker} {long}");
        }
        let mut runner = Runner::new();
        let rx = runner.spawn(
            "auto-turn",
            agent.clone(),
            react::llm::Content::Text("go".to_string()),
            transcript,
            Some(0),
            1_000,
        );
        let events = futures::executor::block_on(async {
            let mut events = Vec::new();
            while let Ok(event) = rx.recv().await {
                let terminal = matches!(event, AgentEvent::Done | AgentEvent::Error(_));
                events.push(event);
                if terminal {
                    break;
                }
            }
            events
        });
        assert!(
            matches!(
                events.first(),
                Some(AgentEvent::AutoCompacted { dropped: 5, before: _, summary })
                    if summary.contains("auto checkpoint")
            ),
            "the first event is the automatic compaction: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::Text(text) if text == "chunk1")),
            "the turn still streams after the checkpoint"
        );
        let session = agent.session();
        match session.messages().first() {
            Some(react::LlmMessage::User { content }) => match content {
                react::llm::Content::Text(text) => {
                    assert!(text.contains("<compacted-summary>"));
                    assert!(text.contains("auto checkpoint"));
                }
                _ => panic!("the checkpoint is text"),
            },
            other => panic!("the checkpoint is a user message: {other:?}"),
        }
        assert!(
            matches!(
                session.messages().get(1),
                Some(react::LlmMessage::Assistant { content }) if content.starts_with("r3")
            ),
            "the retained tail survives the automatic checkpoint"
        );
    }
}
