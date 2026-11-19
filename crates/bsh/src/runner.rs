//! Streaming bridge: runs the agent on a Tokio runtime and forwards events.
//!
//! The shell never awaits a model. A turn is a task on the runner's own Tokio
//! runtime that pushes AgentEvents into an async channel the shell polls from
//! the GPUI foreground executor.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agent::{Agent, StreamToken};
use futures::StreamExt;
use react::llm::{ActionIntent, Content};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

use crate::model::{ChatMessage, Role};

/// Whether one streamed tool result reports a failure.
///
/// The engine renders every result as the shared envelope, whose `status` is
/// the verdict; a transcript written before the envelope is still recognized
/// by its legacy `Error: ` prefix or a payload-level `"success":false`.
fn tool_result_failed(output: &str) -> bool {
    react::tool::ToolResult::text_is_error(output)
}

/// Cap the transcript-facing tool result by UTF-8 bytes, not Unicode scalar count.
fn cap_display_tool_output(output: &str, cap_bytes: usize) -> String {
    // A zero host cap delegates to the engine's documented default budget.
    if cap_bytes == 0 || output.len() <= cap_bytes {
        return output.to_string();
    }

    const MARKER: &str = "\n... [output truncated for display]";
    if cap_bytes <= MARKER.len() {
        return MARKER[..cap_bytes].to_string();
    }

    let mut end = cap_bytes - MARKER.len();
    while end > 0 && !output.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{MARKER}", &output[..end])
}

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
    /// An action turn ended without a successful tool call.
    ///
    /// The engine already refuses a completion claim after a failed attempt and
    /// reports the outcome; this event carries the same fact to the shell so
    /// the reader sees it in the status row instead of only in a log.
    ActionMissed {
        /// Tool calls the turn attempted.
        tool_calls: usize,
        /// Calls that returned a failure.
        failures: usize,
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
    /// The harness auto-injected context/persona into this turn.
    ///
    /// Lands right after the turn frame is injected and before the first token,
    /// so the transcript can draw an inject event naming what the model received.
    ContextInjected {
        /// The injected blocks: persona, runtime context, workspace instructions.
        injections: Vec<crate::model::ContextInjection>,
    },
    /// A steered message was taken by the running turn's next step.
    ///
    /// The ticket is the host identity the message was queued with. It lands
    /// at the step boundary, before that step's first token, so the shell
    /// records the message exactly where the running turn took it.
    Steered {
        /// The host ticket the message was queued with.
        ticket: u64,
    },
    /// The turn ended before taking a steered message, so it was handed back.
    ///
    /// The shell restores the row rather than losing a message the model never
    /// saw. It lands before the terminal event, so a queue drained by the
    /// finished turn already sees it.
    SteerWithdrawn {
        /// The host ticket the message was queued with.
        ticket: u64,
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
    /// When each session's previous turn was prepared, for the time-context's
    /// elapsed span. A session's first turn has no preceding message.
    prepared: HashMap<String, u64>,
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
            prepared: HashMap::new(),
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
    ///
    /// The stored history replays in the model's own order: text, then each
    /// tool call with its result. The runtime snapshot and workspace
    /// instructions are injected by [`Runner::spawn`] after compaction,
    /// immediately before the stream, so the fresh context is never folded
    /// into the checkpoint it was meant to supersede.
    pub(crate) fn seed_session(agent: &Agent, transcript: &[ChatMessage]) {
        let commands = crate::extension::cached_commands();
        Self::seed_session_with(agent, transcript, &commands);
    }

    /// The same seed against an explicit command list.
    ///
    /// Split out so the replay of a `/command` annotation is testable without a
    /// live extension host: the production path is [`Self::seed_session`], which
    /// reads the host's current list once per seed.
    pub(crate) fn seed_session_with(
        agent: &Agent,
        transcript: &[ChatMessage],
        commands: &[crate::extension::CommandSurface],
    ) {
        let mut session = agent.session();
        session.restore_messages(Vec::new());
        for (row_index, message) in transcript.iter().enumerate() {
            match message.role {
                // A stored user row with images replays as the same multimodal
                // content its live turn sent, which is how the /goal attachment
                // message stays readable by a later goal round. An extension
                // command's `/name` annotation replays as the block its live
                // send carried, or a later turn reads a command it cannot see.
                Role::User => {
                    if let Some(content) = crate::state::message_content(message) {
                        session
                            .add_user_content(crate::command_invocation::replay(commands, content));
                    }
                }
                Role::Assistant => {
                    // Reasoning stays in the transcript for the reader, but it
                    // is NOT replayed: a model must never receive a previous
                    // turn's chain of thought or its opaque provider state.
                    // Only the current turn's engine session carries those.
                    if !message.text.is_empty() {
                        session.add_assistant(message.text.clone());
                    }
                    // The tool traffic replays too. Dropping it used to make
                    // every turn re-do the last turn's reads - the model
                    // re-read the same files each turn because the results it
                    // already held were gone - and it moved the request
                    // prefix, so the provider's prompt cache never carried
                    // across turns. The stored outputs are the capped ones the
                    // request sent; the ids are seed-local and only need to
                    // pair call with result inside this history.
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

    /// Start streaming a turn, returning the event receiver.
    /// The tokio runtime bsh drives blocking-capable I/O on.
    ///
    /// The gpui executor has no reactor, so a reqwest call started from a
    /// view has to be handed to this runtime and posted back.
    pub(crate) fn rt(&self) -> &Runtime {
        &self.rt
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn(
        &mut self,
        session_id: &str,
        agent: Arc<Agent>,
        input: Content,
        transcript: Vec<ChatMessage>,
        facts: crate::context::RuntimeFacts,
        auto_retain: Option<usize>,
        tool_output_cap: usize,
        memory_state: crate::context_assembly::MemoryState,
        persona_complete: bool,
        capture: Option<Arc<crate::memory_capture::CaptureHandle>>,
    ) -> async_channel::Receiver<AgentEvent> {
        self.stop(session_id);
        Self::seed_session(&agent, &transcript);

        // The time-context facts, sampled once per turn preparation: the turn
        // number counted from the reader's own prompts, and the span since
        // this session's previous preparation (unavailable on a first turn).
        let turn = transcript
            .iter()
            .filter(|message| message.role == Role::User)
            .count()
            + 1;
        let prepared_at = crate::state::now();
        let elapsed = self
            .prepared
            .insert(session_id.to_string(), prepared_at)
            .map(|previous| prepared_at.saturating_sub(previous));
        let time = crate::context::TimeFacts { turn, elapsed };
        // The agent may outlive a turn, and its capability shims are built
        // once; stamp every call this turn makes with the current turn id.
        agent.set_capability_turn(format!("turn-{turn}"));

        // Owned copies for the spawned task: the capture evidence outlives
        // the method body (issue #343).
        let session = session_id.to_string();
        let (tx, rx) = async_channel::unbounded();
        // The turn's delivery sink, installed before the stream starts: the
        // agent reports a steered ticket at the step that takes it, which is
        // the moment the shell records the message. It belongs to this turn -
        // the next turn builds a fresh agent and installs its own sink.
        let steered = tx.clone();
        agent.on_steered(move |ticket| {
            let _ = steered.try_send(AgentEvent::Steered { ticket });
        });
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
            // What this turn asks for, decided once from the reader's own
            // words before the stream opens. The engine uses it for the action
            // contract (one corrective retry when an actionable turn is
            // answered with prose only) and reports it on the turn outcome; it
            // is never serialized to the provider.
            let text = input.as_text().unwrap_or_default().to_string();
            let mut intent = crate::intent::classify(&text);
            // The lexical classifier is fast but has no semantics; an
            // unclassified message gets one semantic second opinion from the
            // configured model (bounded, cached, and abandoned on any error).
            // Very short messages are greetings and skip the call.
            // A slash-command invocation is already an explicit action; its
            // composed block is not a message to classify semantically.
            if intent == ActionIntent::Unknown
                && text.trim().chars().count() >= 12
                && !text.contains("<extension_command")
            {
                if let Some(classified) = agent.classify_intent_with_model(&text).await {
                    intent = classified;
                }
            }
            agent.set_action_intent(intent);
            // The classifier is a provider request too: its usage joins the
            // turn's totals instead of being billed invisibly.
            if let Some(usage) = agent.take_auxiliary_usage() {
                let _ = tx.send_blocking(AgentEvent::Usage {
                    prompt: u64::from(usage.prompt_tokens),
                    completion: u64::from(usage.completion_tokens),
                    cached: u64::from(usage.cached_tokens),
                });
            }
            // The turn's own context is rendered HERE, on the runner's blocking
            // runtime: the snapshot is pure, but instruction discovery stats the
            // workspace, and the send path must not do file I/O on the UI
            // thread. It is injected after the checkpoint so the fresh snapshot
            // cannot be summarized into the history it supersedes.
            let context = crate::context::TurnContext::from_facts_at(&facts, false, &time);
            // The typed snapshot is built below, from the same facts, and its
            // request blocks are what get injected: no separately rendered
            // copy can drift from the assembled types (issue #411).
            // Surface the auto-injection as a chat event: the persona the build
            // composed, the runtime snapshot and the workspace instructions this
            // turn carried. Recorded on the prompt row, display only.
            {
                let mut injections: Vec<crate::model::ContextInjection> = Vec::new();
                // The TYPED assembly (issue #346): the exact system prompt plus
                // the recall account it was built from. Diagnostics read the
                // account, never search the rendered text.
                let assembly = agent.assemble_prompt(&input).await;
                // Retrieved memory is not part of the system prompt: it rides
                // the provider request's untrusted context channel.
                let persona = assembly.base_prompt.clone();
                let recall =
                    crate::context_assembly::MemoryRecallInfo::from_agent(&assembly.memory);

                // The immutable snapshot is the source of truth: one typed
                // block per source, each carrying its trust boundary and
                // provenance to serialization (issue #346).
                let mut blocks = vec![crate::context_assembly::ContextBlock {
                    source: crate::context_assembly::ContextSource::Persona,
                    trust: crate::context_assembly::ContextSource::Persona.trust(),
                    text: assembly.base_prompt.clone(),
                    token_estimate: agent::agent::context::estimate_tokens(&assembly.base_prompt),
                    provenance: Some("persona".to_string()),
                }];
                if !context.snapshot.is_empty() {
                    blocks.push(crate::context_assembly::ContextBlock {
                        source: crate::context_assembly::ContextSource::HostRuntime,
                        trust: crate::context_assembly::ContextSource::HostRuntime.trust(),
                        text: context.snapshot.clone(),
                        token_estimate: agent::agent::context::estimate_tokens(&context.snapshot),
                        provenance: Some("runtime-context".to_string()),
                    });
                }
                if !context.time_context.is_empty() {
                    blocks.push(crate::context_assembly::ContextBlock {
                        source: crate::context_assembly::ContextSource::TimeContext,
                        trust: crate::context_assembly::ContextSource::TimeContext.trust(),
                        text: context.time_context.clone(),
                        token_estimate: agent::agent::context::estimate_tokens(&context.time_context),
                        provenance: Some("time-context".to_string()),
                    });
                }
                if let Some(instructions) = &context.instructions {
                    blocks.push(crate::context_assembly::ContextBlock {
                        source: crate::context_assembly::ContextSource::WorkspaceInstructions,
                        trust: crate::context_assembly::ContextSource::WorkspaceInstructions.trust(),
                        text: instructions.clone(),
                        token_estimate: agent::agent::context::estimate_tokens(instructions),
                        provenance: Some("AGENTS.md".to_string()),
                    });
                }
                if let Some(block) =
                    crate::context_assembly::memory_block(&recall, assembly.memory.block.as_deref())
                {
                    blocks.push(block);
                }
                // Extension instructions are task-scoped (issue #436): an
                // installed package is not a request for its workflow guide.
                // The same selection feeds the blocks, the manifest omissions
                // and the report, so the request and diagnostics cannot
                // disagree about what was included and why.
                let task_text = input.as_text().unwrap_or_default();
                let tool_names = agent.capability_tool_names();
                let extension_plan = crate::extension::activation::activate(
                    &agent.conditional_prompts(),
                    task_text,
                    &tool_names,
                );
                let mut extension_omissions = Vec::new();
                for active in &extension_plan.active {
                    for selected in &active.prompts {
                        let prompt = &selected.prompt;
                        let text = crate::extension::activation::render(active, selected);
                        blocks.push(crate::context_assembly::ContextBlock {
                            source: crate::context_assembly::ContextSource::Extension,
                            trust: crate::context_assembly::ContextSource::Extension.trust(),
                            token_estimate: agent::agent::context::estimate_tokens(&text),
                            provenance: Some(format!(
                                "ext:{}@{}#{}#{}",
                                active.owner, active.version, prompt.id, prompt.kind
                            )),
                            text: text.clone(),
                        });
                        injections.push(crate::model::ContextInjection {
                            kind: "extension".to_string(),
                            content: text,
                            // Name the contribution, not only the package: two
                            // contributions of one extension must not draw two
                            // indistinguishable rows (issue #436 review).
                            source: format!(
                                "{}@{}#{}",
                                active.owner, active.version, prompt.id
                            ),
                        });
                        log::debug!(
                            target: "bsh::context",
                            "extension context active: {}@{}#{} kind={} reason={} bytes={}",
                            active.owner,
                            active.version,
                            prompt.id,
                            prompt.kind,
                            selected.reason,
                            prompt.text.len()
                        );
                    }
                }
                for omission in &extension_plan.omissions {
                    extension_omissions.push(crate::context_assembly::ContextOmission {
                        source: crate::context_assembly::ContextSource::Extension,
                        provenance: format!(
                            "ext:{}@{}#{}#{}",
                            omission.owner, omission.version, omission.contribution, omission.kind
                        ),
                        reason: omission.reason.clone(),
                        tokens: omission.bytes / 4,
                    });
                    log::debug!(
                        target: "bsh::context",
                        "extension context omitted: {}@{}#{} reason={}",
                        omission.owner,
                        omission.version,
                        omission.contribution,
                        omission.reason
                    );
                }
                let instruction_sources = context
                    .instructions
                    .as_deref()
                    .map(|_| crate::instructions::sources(std::path::Path::new(&facts.cwd)).join(","));
                // A content identity is opt-in: the key never lives in the
                // repository, so logs cannot be dictionary-attacked by default.
                let content_key = std::env::var("BSH_CONTEXT_CONTENT_KEY")
                    .ok()
                    .filter(|value| !value.is_empty())
                    .map(String::into_bytes);
                let content_key_id = std::env::var("BSH_CONTEXT_CONTENT_KEY_ID")
                    .ok()
                    .filter(|value| !value.is_empty());
                // The persona is private text, so its version input is a keyed
                // digest when a key is configured, and a coarse length label
                // otherwise: the log never carries an unkeyed persona hash.
                let persona_version = match content_key.as_deref() {
                    Some(key) => crate::context_assembly::keyed_digest(key, &assembly.base_prompt),
                    None => format!(
                        "chars:{}:tokens:{}",
                        assembly.base_prompt.chars().count(),
                        agent::agent::context::estimate_tokens(&assembly.base_prompt)
                    ),
                };
                let system_prompt_version = match content_key.as_deref() {
                    Some(key) => crate::context_assembly::keyed_digest(
                        key,
                        &format!(
                            "{}\n{}",
                            assembly.base_prompt,
                            context.instructions.as_deref().unwrap_or("")
                        ),
                    ),
                    None => format!(
                        "prompt-chars:{}",
                        assembly.base_prompt.chars().count()
                            + context
                                .instructions
                                .as_deref()
                                .map(str::chars)
                                .map(Iterator::count)
                                .unwrap_or(0)
                    ),
                };
                let runtime_config = crate::context_assembly::digest_text(
                    &serde_json::json!({
                        "temperature": agent.config().temperature,
                        "max_tokens": agent.config().max_tokens,
                        "reasoning_effort": agent.config().reasoning_effort,
                        "api_mode": agent.config().api_mode,
                    })
                    .to_string(),
                );
                // Identity inputs are derived from the actual advertised tools,
                // adapter and memory revision, not hard-coded labels (#432).
                let tool_schema_digest = crate::context_assembly::digest_text(
                    &serde_json::to_string(&agent.all_tools()).unwrap_or_default(),
                );
                let memory_revision = (!recall.records.is_empty()).then(|| {
                    crate::context_assembly::digest_text(&recall.records.join(","))
                });
                let versions = crate::context_assembly::SourceVersions {
                    persona: Some(persona_version),
                    workspace_instructions: context.instructions.as_deref().map(|text| {
                        format!(
                            "{}:{}",
                            crate::context_assembly::digest_text(text),
                            instruction_sources.as_deref().unwrap_or("-")
                        )
                    }),
                    memory_records: recall.records.clone(),
                    tool_guidance: Some(format!("tools:{tool_schema_digest}")),
                    provider: Some(agent.config().model.clone()),
                    provider_adapter: Some(format!(
                        "{}:{}:react-{}",
                        agent
                            .config()
                            .model
                            .split_once('/')
                            .map(|(vendor, _)| vendor)
                            .unwrap_or("openai"),
                        agent.config().api_mode,
                        react::llm::ADAPTER_VERSION
                    )),
                    provider_endpoint: Some(format!(
                        "endpoint:{}",
                        crate::context_assembly::digest_text(&agent.config().base_url)
                    )),
                    api_mode: Some(agent.config().api_mode.clone()),
                    tool_schema: Some(tool_schema_digest),
                    tool_schema_revision: Some("tool-definitions-v1".to_string()),
                    memory_revision,
                    memory_schema: Some(agent::memory::RECORD_SCHEMA.to_string()),
                    system_prompt: Some(system_prompt_version),
                    runtime_config: Some(runtime_config),
                    session: Some(session.clone()),
                    session_state: Some(format!("messages:{}", transcript.len())),
                    turn: Some(time.turn as u64),
                };
                let snapshot = crate::context_assembly::AssemblySnapshot::build(
                    blocks,
                    versions,
                    content_key
                        .as_deref()
                        .map(|key| (key, content_key_id.as_deref())),
                )
                .with_omissions(extension_omissions);
                // Inject from the immutable snapshot, after the checkpoint and
                // before the first request. The same snapshot becomes the
                // typed request the engine attaches to every provider call
                // (issue #421).
                crate::context::inject_snapshot(&agent, &snapshot);
                agent.set_provider_request(Some(snapshot.typed_provider_request()));

                // Per-request assembly diagnostics (issues #336/#346): counts,
                // labels and token estimates derived from the typed assembly,
                // never raw memory or workspace content.
                let mut report = crate::context_assembly::assemble_report(
                    persona_complete,
                    &assembly.base_prompt,
                    &persona,
                    &context.snapshot,
                    &context.time_context,
                    context.instructions.as_deref(),
                    &memory_state,
                    &recall,
                    agent::agent::context::estimate_tokens(input.as_text().unwrap_or_default()),
                );
                report.composition_fingerprint = snapshot.fingerprint().composition.clone();
                report.content_fingerprint = snapshot.fingerprint().content.clone();
                // Extension inclusion/omission, from the same selection that
                // built the blocks and the manifest (issue #436).
                for active in &extension_plan.active {
                    for selected in &active.prompts {
                        let bytes = selected.prompt.text.len();
                        report.estimated_tokens =
                            report.estimated_tokens.saturating_add(bytes / 4);
                        report.sources.push(crate::context_assembly::SourceStatus {
                            source: crate::context_assembly::ContextSource::Extension,
                            included: true,
                            reason: Some(format!(
                                "{}@{}#{}: {}",
                                active.owner,
                                active.version,
                                selected.prompt.id,
                                selected.reason
                            )),
                            tokens: bytes / 4,
                        });
                    }
                }
                for omission in snapshot.omissions() {
                    report.sources.push(crate::context_assembly::SourceStatus {
                        source: crate::context_assembly::ContextSource::Extension,
                        included: false,
                        reason: Some(format!(
                            "{}: {}",
                            omission.provenance, omission.reason
                        )),
                        tokens: omission.tokens,
                    });
                }
                log::info!(
                    target: "bsh::context",
                    "context assembly: persona_complete={} memory={} retrieval_hits={} est_tokens={} snapshot_tokens={} fingerprint={} snapshot_fingerprint={} content_fingerprint={} retrieval_skip={}",
                    report.persona_complete,
                    report.memory.label(),
                    report.retrieval_hits,
                    report.estimated_tokens,
                    snapshot.total_tokens(),
                    report.composition_fingerprint.get(..16).unwrap_or(&report.composition_fingerprint),
                    snapshot.fingerprint().composition_short(),
                    report.content_fingerprint.as_deref().map(|value| &value[..16.min(value.len())]).unwrap_or("-"),
                    report.retrieval_skip.as_deref().unwrap_or("none")
                );
                // The memory block's provenance, read from the snapshot itself.
                if let Some(block) = snapshot.block(crate::context_assembly::ContextSource::RetrievedMemory) {
                    log::debug!(
                        target: "bsh::context",
                        "retrieved memory block: trust={:?} tokens={} provenance={}",
                        block.trust,
                        block.token_estimate,
                        block.provenance.as_deref().unwrap_or("-")
                    );
                }
                // Per-block trust and provenance, read from the snapshot itself.
                for block in snapshot.blocks() {
                    log::debug!(
                        target: "bsh::context",
                        "context block: {} trust={:?} tokens={} provenance={}",
                        block.source.label(),
                        block.trust,
                        block.token_estimate,
                        block.provenance.as_deref().unwrap_or("-")
                    );
                }
                for status in &report.sources {
                    log::debug!(
                        target: "bsh::context",
                        "context source: {} included={} trust={:?} tokens={} reason={}",
                        status.source.label(),
                        status.included,
                        status.source.trust(),
                        status.tokens,
                        status.reason.as_deref().unwrap_or("-")
                    );
                }

                if !persona.trim().is_empty() {
                    injections.push(crate::model::ContextInjection {
                        kind: "persona".to_string(),
                        content: persona,
                        source: "persona".to_string(),
                    });
                }
                if !context.snapshot.is_empty() {
                    injections.push(crate::model::ContextInjection {
                        kind: "context".to_string(),
                        content: context.snapshot.clone(),
                        source: "runtime-context".to_string(),
                    });
                }
                if !context.time_context.is_empty() {
                    injections.push(crate::model::ContextInjection {
                        kind: "time".to_string(),
                        content: context.time_context.clone(),
                        source: "time-context".to_string(),
                    });
                }
                if let Some(instructions) = &context.instructions {
                    // Name the producer the way the reference does: the file the
                    // instructions came from (AGENTS.md), not a generic kind.
                    let source =
                        crate::instructions::sources(std::path::Path::new(&facts.cwd)).join(", ");
                    injections.push(crate::model::ContextInjection {
                        kind: "instructions".to_string(),
                        content: instructions.clone(),
                        source,
                    });
                }
                if !injections.is_empty() {
                    let _ = tx.send_blocking(AgentEvent::ContextInjected { injections });
                }
            }

            let mut tool_calls = 0usize;
            let mut tool_failures = 0usize;
            let mut tool_successes = 0usize;
            // The turn's answer text, accumulated for the capture coordinator
            // (issue #343): only a COMPLETED turn offers evidence.
            let mut answer = String::new();

            // The terminal event is held back until the mailbox has been handed
            // back: a steer the turn never reached another step for must be
            // restored BEFORE the shell starts the next turn from the queue.
            let mut terminal_event = None;
            {
                let mut stream = agent.stream(input);
                while let Some(item) = stream.next().await {
                    if stop_task.load(Ordering::SeqCst) {
                        break;
                    }
                    let terminal =
                        matches!(&item, Ok(StreamToken::Done | StreamToken::Stopped) | Err(_));
                    let event = match item {
                        Ok(StreamToken::Text(text)) => {
                            answer.push_str(&text);
                            AgentEvent::Text(text)
                        }
                        Ok(StreamToken::ReasoningContent(text)) => AgentEvent::Reasoning(text),
                        // The engine keeps the state in its own turn-local
                        // session; the transcript does not persist or replay it.
                        Ok(StreamToken::ReasoningState(_)) => continue,
                        Ok(StreamToken::ToolCall { name, args, .. }) => {
                            tool_calls += 1;
                            AgentEvent::ToolCall {
                                name,
                                args: args.to_string(),
                            }
                        }
                        Ok(StreamToken::ToolResult { name, output, ms }) => {
                            if tool_result_failed(&output) {
                                tool_failures += 1;
                            } else {
                                tool_successes += 1;
                            }
                            let output = cap_display_tool_output(&output, tool_output_cap);
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
                    if terminal {
                        terminal_event = Some(event);
                        break;
                    }
                    if tx.send_blocking(event).is_err() {
                        break;
                    }
                }
            }
            for ticket in agent.take_undelivered_steers() {
                if tx
                    .send_blocking(AgentEvent::SteerWithdrawn { ticket })
                    .is_err()
                {
                    break;
                }
            }
            // An action turn that called no tool, or whose every call failed,
            // ends on prose the reader cannot trust as work. The engine refuses
            // the completion claim and logs it; this carries the same fact to
            // the status row so it is visible where the answer is read.
            if matches!(terminal_event, Some(AgentEvent::Done))
                && intent == ActionIntent::Action
                && tool_successes == 0
            {
                let _ = tx.send_blocking(AgentEvent::ActionMissed {
                    tool_calls,
                    failures: tool_failures,
                });
            }
            // Governed capture at the completed-turn boundary (issue #343):
            // only a Done turn offers evidence - a stopped or failed turn does
            // not - and the capture runs ASYNCHRONOUSLY beside the terminal
            // event, so a capture failure never invalidates the turn.
            if matches!(terminal_event, Some(AgentEvent::Done)) {
                if let Some(capture) = &capture {
                    let evidence = agent::memory::capture::TurnEvidence {
                        user_text: text.clone(),
                        assistant_text: answer.clone(),
                        session: Some(session.clone()),
                        turn: Some(format!("t{turn}")),
                        tool_evidence: Vec::new(),
                    };
                    capture.spawn(&tokio::runtime::Handle::current(), evidence);
                }
            }
            if let Some(event) = terminal_event {
                let _ = tx.send_blocking(event);
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

    #[test]
    fn display_tool_output_cap_counts_bytes_and_preserves_utf8() {
        let output = "界".repeat(100);
        let capped = cap_display_tool_output(&output, 64);
        assert!(capped.len() <= 64, "the cap is in bytes: {}", capped.len());
        assert!(capped.is_char_boundary(capped.len()));
        assert!(capped.contains("[output truncated for display]"));
    }

    #[test]
    fn a_failed_tool_result_is_recognised() {
        assert!(tool_result_failed("Error: permission denied"));
        assert!(tool_result_failed(
            "{\"success\":false,\"stderr\":\"boom\"}"
        ));
        assert!(!tool_result_failed("{\"success\":true}"));
        assert!(!tool_result_failed("plain output"));
    }

    #[test]
    fn zero_display_cap_leaves_the_engine_default_result_untouched() {
        let output = "界".repeat(100);
        assert_eq!(cap_display_tool_output(&output, 0), output);
    }

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
            Ok(summary_response(
                "## Primary Request and Intent\n- finish the turn\n\n\
                 ## Key Technical Concepts\n- compaction\n\n\
                 ## Files and Code\n- runner\n\n\
                 ## Errors and Fixes\n- none\n\n\
                 ## Pending Jobs\n- none\n\n\
                 ## Current Work\n- auto checkpoint\n\n\
                 ## Next Step\n- stream\n\n\
                 ## Critical Context\n- auto checkpoint",
            ))
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
        let long = "detail ".repeat(200);
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
            crate::context::RuntimeFacts {
                cwd: String::new(),
                confined: true,
                sandbox_reason: None,
                permission: "workspace-write".to_string(),
                preset: "standard".to_string(),
                presets_disabled: false,
            },
            Some(0),
            1_000,
            crate::context_assembly::MemoryState::Disabled,
            false,
            None,
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
        let messages = session.messages();
        // The turn's frame is injected at the HEAD of the request (the stable
        // prefix a provider cache can hit across turns), so the checkpoint
        // follows the snapshot instead of occupying slot 0.
        let checkpoint = messages
            .iter()
            .position(|message| {
                matches!(
                    message,
                    react::LlmMessage::User { content }
                        if content.as_text().is_some_and(|text| {
                            text.contains("<compacted-summary>")
                                && text.contains("auto checkpoint")
                        })
                )
            })
            .expect("the checkpoint is a user message");
        assert!(
            messages[..checkpoint].iter().all(|message| matches!(
                message,
                react::LlmMessage::Developer { content }
                    if content.starts_with(crate::context::RUNTIME_CONTEXT_PREAMBLE)
                        || content.starts_with(crate::context::TIME_CONTEXT_PREAMBLE)
            )),
            "only the injected developer frame precedes the checkpoint: {messages:?}"
        );
        assert!(
            matches!(
                messages.get(checkpoint + 1),
                Some(react::LlmMessage::Assistant { content }) if content.starts_with("r3")
            ),
            "the retained tail survives the automatic checkpoint"
        );
    }

    /// A vendor that records the text row and role of the request the engine
    /// builds.
    ///
    /// Recording the session at the stream call proves the injected context is
    /// what the provider would receive, not merely a value the shell stored -
    /// including that context is a system row and the reader's line is the only
    /// user row (issue #434).
    struct ContextCapturingLlm(Arc<std::sync::Mutex<Vec<(&'static str, String)>>>);

    /// The (role, text) rows one turn's provider request carried.
    type CapturedRows = Arc<std::sync::Mutex<Vec<(&'static str, String)>>>;

    /// The row texts, in wire order.
    fn captured_text(rows: &CapturedRows) -> Vec<String> {
        rows.lock()
            .unwrap()
            .iter()
            .map(|(_, text)| text.clone())
            .collect()
    }

    /// The row roles, in wire order.
    fn captured_roles(rows: &CapturedRows) -> Vec<&'static str> {
        rows.lock().unwrap().iter().map(|(role, _)| *role).collect()
    }

    #[async_trait::async_trait]
    impl agent::LlmClient<agent::agent::AgentSession, agent::agent::AgentReactContext>
        for ContextCapturingLlm
    {
        async fn complete(
            &self,
            _persona: Option<String>,
            _req: react::llm::LlmRequest,
            _session: &mut agent::agent::AgentSession,
            _context: &mut agent::agent::AgentReactContext,
        ) -> react::llm::LlmResponseResult {
            Ok(summary_response("ok"))
        }

        async fn stream_complete(
            &self,
            _persona: Option<String>,
            _req: react::llm::LlmRequest,
            session: &mut agent::agent::AgentSession,
            _context: &mut agent::agent::AgentReactContext,
        ) -> Result<react::llm::TokenStream, react::llm::LlmError> {
            *self.0.lock().unwrap() = session
                .messages()
                .iter()
                .filter_map(|message| match message {
                    react::LlmMessage::System { content } => Some(("system", content.clone())),
                    react::LlmMessage::Developer { content } => {
                        Some(("developer", content.clone()))
                    }
                    react::LlmMessage::User { content } => {
                        content.as_text().map(|text| ("user", text.to_string()))
                    }
                    _ => None,
                })
                .collect();
            // A real turn says something; an empty stream would trip the
            // engine's blank-answer retry and add a harness note this test is
            // not about.
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(react::llm::StreamToken::Text("ok".to_string())),
                Ok(react::llm::StreamToken::Done),
            ])))
        }

        fn provider_name(&self) -> &'static str {
            "context-capturing-mock"
        }
    }

    fn workspace(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bsh-runner-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.subsec_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch workspace");
        dir
    }

    /// Records the typed provider request the engine actually sent.
    struct RequestCapturingLlm(Arc<std::sync::Mutex<Vec<react::llm::ProviderRequest>>>);

    #[async_trait::async_trait]
    impl agent::LlmClient<agent::agent::AgentSession, agent::agent::AgentReactContext>
        for RequestCapturingLlm
    {
        async fn complete(
            &self,
            _persona: Option<String>,
            request: react::llm::LlmRequest,
            _session: &mut agent::agent::AgentSession,
            _context: &mut agent::agent::AgentReactContext,
        ) -> react::llm::LlmResponseResult {
            if let Some(snapshot) = request.provider_request.clone() {
                self.0.lock().unwrap().push(snapshot);
            }
            Ok(summary_response("ok"))
        }

        async fn stream_complete(
            &self,
            _persona: Option<String>,
            request: react::llm::LlmRequest,
            _session: &mut agent::agent::AgentSession,
            _context: &mut agent::agent::AgentReactContext,
        ) -> Result<react::llm::TokenStream, react::llm::LlmError> {
            if let Some(snapshot) = request.provider_request.clone() {
                self.0.lock().unwrap().push(snapshot);
            }
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(react::llm::StreamToken::Text("ok".to_string())),
                Ok(react::llm::StreamToken::Done),
            ])))
        }

        fn provider_name(&self) -> &'static str {
            "request-capturing-mock"
        }
    }

    /// A provider that scripts the reasoning blocks and tool calls of a
    /// two-step tool loop, then answers the next turn. It records every typed
    /// request so the test can assert exactly what each step replayed.
    struct ReasoningLifecycleLlm {
        seen: Arc<std::sync::Mutex<Vec<react::llm::ProviderRequest>>>,
        step: std::sync::atomic::AtomicUsize,
    }

    fn lifecycle_state(signature: &str) -> react::llm::ReasoningState {
        react::llm::ReasoningState::new(
            "anthropic",
            serde_json::json!({ "type": "thinking", "signature": signature }),
        )
    }

    /// The reasoning blocks a request carried, in wire order.
    fn reasoning_blocks(
        messages: &[react::LlmMessage],
    ) -> Vec<(String, Option<react::llm::ReasoningState>)> {
        messages
            .iter()
            .filter_map(|message| match message {
                react::LlmMessage::AssistantReasoning {
                    content,
                    provider_state,
                } => Some((content.clone(), provider_state.clone())),
                _ => None,
            })
            .collect()
    }

    #[async_trait::async_trait]
    impl agent::LlmClient<agent::agent::AgentSession, agent::agent::AgentReactContext>
        for ReasoningLifecycleLlm
    {
        async fn complete(
            &self,
            _persona: Option<String>,
            _req: react::llm::LlmRequest,
            _session: &mut agent::agent::AgentSession,
            _context: &mut agent::agent::AgentReactContext,
        ) -> react::llm::LlmResponseResult {
            Err(react::llm::LlmError::Other(
                "this fixture streams".to_string(),
            ))
        }

        async fn stream_complete(
            &self,
            _persona: Option<String>,
            request: react::llm::LlmRequest,
            _session: &mut agent::agent::AgentSession,
            _context: &mut agent::agent::AgentReactContext,
        ) -> Result<react::llm::TokenStream, react::llm::LlmError> {
            if let Some(snapshot) = request.provider_request.clone() {
                self.seen.lock().unwrap().push(snapshot);
            }
            let index = self.step.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let tokens: Vec<Result<react::llm::StreamToken, react::llm::LlmError>> = match index {
                0 => vec![
                    Ok(react::llm::StreamToken::ReasoningContent(
                        "alpha".to_string(),
                    )),
                    Ok(react::llm::StreamToken::ReasoningState(lifecycle_state(
                        "sig-alpha",
                    ))),
                    Ok(react::llm::StreamToken::ToolCall {
                        name: "echo".to_string(),
                        args: serde_json::json!({ "n": 1 }),
                        id: Some("call-alpha".to_string()),
                    }),
                    Ok(react::llm::StreamToken::Done),
                ],
                1 => vec![
                    Ok(react::llm::StreamToken::ReasoningContent(
                        "beta".to_string(),
                    )),
                    Ok(react::llm::StreamToken::ReasoningState(lifecycle_state(
                        "sig-beta",
                    ))),
                    Ok(react::llm::StreamToken::ToolCall {
                        name: "echo".to_string(),
                        args: serde_json::json!({ "n": 2 }),
                        id: Some("call-beta".to_string()),
                    }),
                    Ok(react::llm::StreamToken::Done),
                ],
                2 => vec![
                    Ok(react::llm::StreamToken::Text("finished".to_string())),
                    Ok(react::llm::StreamToken::Done),
                ],
                _ => vec![
                    Ok(react::llm::StreamToken::Text("second answer".to_string())),
                    Ok(react::llm::StreamToken::Done),
                ],
            };
            Ok(Box::pin(futures::stream::iter(tokens)))
        }

        fn supports_tools(&self) -> bool {
            true
        }

        fn provider_name(&self) -> &'static str {
            "reasoning-lifecycle-mock"
        }
    }

    /// Records the tool names the provider request actually carried.
    struct AdmissionCapturingLlm(Arc<std::sync::Mutex<Vec<Vec<String>>>>);

    #[async_trait::async_trait]
    impl agent::LlmClient<agent::agent::AgentSession, agent::agent::AgentReactContext>
        for AdmissionCapturingLlm
    {
        async fn complete(
            &self,
            _persona: Option<String>,
            _req: react::llm::LlmRequest,
            _session: &mut agent::agent::AgentSession,
            _context: &mut agent::agent::AgentReactContext,
        ) -> react::llm::LlmResponseResult {
            Ok(summary_response("ok"))
        }

        async fn stream_complete(
            &self,
            _persona: Option<String>,
            request: react::llm::LlmRequest,
            _session: &mut agent::agent::AgentSession,
            _context: &mut agent::agent::AgentReactContext,
        ) -> Result<react::llm::TokenStream, react::llm::LlmError> {
            let names = request
                .provider_request
                .as_ref()
                .map(|request| request.tools.iter().map(|tool| tool.name.clone()).collect())
                .unwrap_or_default();
            self.0.lock().unwrap().push(names);
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(react::llm::StreamToken::Text("ok".to_string())),
                Ok(react::llm::StreamToken::Done),
            ])))
        }

        fn provider_name(&self) -> &'static str {
            "admission-capturing-mock"
        }
    }

    fn capturing_agent(seen: CapturedRows) -> Arc<agent::Agent> {
        let mut provider = agent::agent::agentic::LlmProvider::new();
        provider.register_vendor("mock".to_string(), Box::new(ContextCapturingLlm(seen)));
        Arc::new(agent::Agent::new(
            agent::AgentConfig {
                model: "mock-model".to_string(),
                ..Default::default()
            },
            Arc::new(provider),
        ))
    }

    fn drain(receiver: async_channel::Receiver<AgentEvent>) {
        futures::executor::block_on(async {
            while let Ok(event) = receiver.recv().await {
                if matches!(event, AgentEvent::Done | AgentEvent::Error(_)) {
                    break;
                }
            }
        });
    }

    /// The first context row of a turn is the runtime snapshot; the workspace
    /// instructions follow it; the reader's own line comes last. Context rows
    /// are system messages, never user messages (issue #434).
    #[test]
    fn the_first_context_row_of_a_turn_carries_the_live_facts() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let agent = capturing_agent(seen.clone());
        let root = workspace("context");
        std::fs::write(root.join("AGENTS.md"), "workspace rule").expect("the file");
        let facts = crate::context::RuntimeFacts {
            cwd: root.to_string_lossy().to_string(),
            confined: true,
            sandbox_reason: None,
            permission: "workspace-write".to_string(),
            preset: "standard".to_string(),
            presets_disabled: false,
        };
        let mut runner = Runner::new();
        let receiver = runner.spawn(
            "context-turn",
            agent,
            react::llm::Content::Text("go".to_string()),
            Vec::new(),
            facts,
            None,
            1_000,
            crate::context_assembly::MemoryState::Disabled,
            false,
            None,
        );
        drain(receiver);
        let rows = captured_text(&seen);
        assert_eq!(
            rows.len(),
            4,
            "snapshot, time, instructions, input: {rows:?}"
        );
        assert_eq!(
            captured_roles(&seen),
            vec!["developer", "developer", "developer", "user"],
            "context is developer instruction; only the reader's line is a user row"
        );
        assert!(
            rows[0].starts_with(crate::context::RUNTIME_CONTEXT_PREAMBLE),
            "the first context row is the snapshot: {}",
            rows[0]
        );
        assert!(
            rows[1].starts_with(crate::context::TIME_CONTEXT_PREAMBLE),
            "the time-context follows the snapshot: {}",
            rows[1]
        );
        assert!(
            rows[0].contains(&format!("Current working directory: {root:?}.")),
            "{}",
            rows[0]
        );
        assert!(rows[0].contains("Sandbox: confined to the workspace."));
        assert!(rows[0].contains("Permission: workspace-write."));
        assert!(rows[0].contains("Agent preset: standard."));
        assert!(
            rows[2].contains("Instructions from: AGENTS.md") && rows[2].contains("workspace rule"),
            "the instruction message follows the frame: {}",
            rows[2]
        );
        // Issue #411: the request text is rendered from the typed snapshot, so
        // every injected block carries its source and trust boundary.
        assert!(
            rows[0]
                .ends_with("<!-- bsh-context source=\"host-runtime\" trust=\"host-authority\" -->"),
            "the runtime block names its host authority: {}",
            rows[0]
        );
        assert!(
            rows[1]
                .ends_with("<!-- bsh-context source=\"time-context\" trust=\"host-authority\" -->"),
            "the time block names its host authority: {}",
            rows[1]
        );
        assert!(
            rows[2].ends_with(
                "<!-- bsh-context source=\"workspace-instructions\" trust=\"untrusted-data\" -->"
            ),
            "the workspace block is explicitly untrusted data: {}",
            rows[2]
        );
        // Sources that stay in the system prompt are still named in the
        // request manifest, so the adapter boundary can tell them apart.
        assert!(
            rows[0].contains("BSH-CONTEXT-MANIFEST-V1"),
            "the request carries the typed manifest: {}",
            rows[0]
        );
        assert!(
            rows[0].contains("\"source\":\"persona\""),
            "the persona is named in the manifest"
        );
        assert!(
            rows[0].contains("\"source\":\"workspace-instructions\""),
            "the workspace source is named in the manifest"
        );
        assert_eq!(rows[3], "go", "the reader's line is last and unframed");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The captured shape of issue #435: a plain greeting must not advertise
    /// the workspace roster. Discovery (and the skill loader) is all it gets.
    #[test]
    fn a_plain_chat_request_carries_only_the_discovery_surface() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut provider = agent::agent::agentic::LlmProvider::new();
        provider.register_vendor(
            "mock".to_string(),
            Box::new(AdmissionCapturingLlm(seen.clone())),
        );
        let mut agent = agent::Agent::new(
            agent::AgentConfig {
                model: "mock-model".to_string(),
                ..Default::default()
            },
            Arc::new(provider),
        );
        agent.add_tool(Arc::new(agent::tools::FunctionTool::new(
            "read_file",
            "read a file",
            serde_json::json!({"type": "object"}),
            |_args| Ok(serde_json::json!({"ok": true})),
        )));
        agent.set_initial_tool_scope(react::tool::admission::InitialToolScope::new(
            ["read_file"],
            Vec::<String>::new(),
        ));
        let root = workspace("admission-chat");
        let facts = crate::context::RuntimeFacts {
            cwd: root.to_string_lossy().to_string(),
            confined: true,
            sandbox_reason: None,
            permission: "workspace-write".to_string(),
            preset: "standard".to_string(),
            presets_disabled: false,
        };
        let mut runner = Runner::new();
        let receiver = runner.spawn(
            "admission-chat-turn",
            Arc::new(agent),
            react::llm::Content::Text("hello 8888".to_string()),
            Vec::new(),
            facts,
            None,
            1_000,
            crate::context_assembly::MemoryState::Disabled,
            false,
            None,
        );
        drain(receiver);
        let requests = seen.lock().unwrap();
        let names = requests.last().expect("the provider request was captured");
        assert!(
            names.iter().any(|name| name == "find_tool"),
            "discovery is advertised: {names:?}"
        );
        assert!(
            !names.iter().any(|name| name == "read_file"),
            "the workspace reader is not advertised for a greeting: {names:?}"
        );
        assert!(names.len() <= 2, "only discovery surfaces: {names:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A work request carries the workspace core initially; a specialist is
    /// withheld until discovery (issue #435).
    #[test]
    fn a_work_request_carries_the_core_and_withholds_a_specialist() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut provider = agent::agent::agentic::LlmProvider::new();
        provider.register_vendor(
            "mock".to_string(),
            Box::new(AdmissionCapturingLlm(seen.clone())),
        );
        let mut agent = agent::Agent::new(
            agent::AgentConfig {
                model: "mock-model".to_string(),
                ..Default::default()
            },
            Arc::new(provider),
        );
        for name in ["read_file", "federation_search"] {
            agent.add_tool(Arc::new(agent::tools::FunctionTool::new(
                name,
                "a tool",
                serde_json::json!({"type": "object"}),
                |_args| Ok(serde_json::json!({"ok": true})),
            )));
        }
        agent.set_initial_tool_scope(react::tool::admission::InitialToolScope::new(
            ["read_file"],
            Vec::<String>::new(),
        ));
        let root = workspace("admission-work");
        let facts = crate::context::RuntimeFacts {
            cwd: root.to_string_lossy().to_string(),
            confined: true,
            sandbox_reason: None,
            permission: "workspace-write".to_string(),
            preset: "standard".to_string(),
            presets_disabled: false,
        };
        let mut runner = Runner::new();
        let receiver = runner.spawn(
            "admission-work-turn",
            Arc::new(agent),
            react::llm::Content::Text("fix the retry logic".to_string()),
            Vec::new(),
            facts,
            None,
            1_000,
            crate::context_assembly::MemoryState::Disabled,
            false,
            None,
        );
        drain(receiver);
        let requests = seen.lock().unwrap();
        let names = requests.last().expect("the provider request was captured");
        assert!(
            names.iter().any(|name| name == "read_file"),
            "the core reader is initial: {names:?}"
        );
        assert!(
            names.iter().any(|name| name == "find_tool"),
            "discovery is advertised next to the core: {names:?}"
        );
        assert!(
            !names.iter().any(|name| name == "federation_search"),
            "the specialist stays out of the initial array: {names:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    fn devkit_prompt() -> agent::agent::ConditionalPrompt {
        agent::agent::ConditionalPrompt {
            owner: "com.example.devkit".to_string(),
            owner_name: "Extension DevKit".to_string(),
            version: "1.2.0".to_string(),
            trust: "community".to_string(),
            id: "guide".to_string(),
            kind: "prompt".to_string(),
            description: "Extension DevKit workflow".to_string(),
            text: "DEVKIT-BODY: design, build, validate, publish.".to_string(),
            scope: Vec::new(),
            triggers: vec![
                "extension".to_string(),
                "devkit".to_string(),
                "design".to_string(),
                "build".to_string(),
                "validate".to_string(),
                "publish".to_string(),
            ],
            commands: vec!["devkit__scaffold".to_string()],
            tools: vec!["devkit__scaffold".to_string(), "scaffold".to_string()],
            tool_namespace: "devkit".to_string(),
            diagnostic: None,
        }
    }

    fn request_capturing_agent(
        seen: Arc<std::sync::Mutex<Vec<react::llm::ProviderRequest>>>,
    ) -> Arc<agent::Agent> {
        let mut provider = agent::agent::agentic::LlmProvider::new();
        provider.register_vendor("mock".to_string(), Box::new(RequestCapturingLlm(seen)));
        Arc::new(agent::Agent::new(
            agent::AgentConfig {
                model: "mock-model".to_string(),
                ..Default::default()
            },
            Arc::new(provider),
        ))
    }

    fn spawn_turn(
        runner: &mut Runner,
        id: &str,
        agent: Arc<agent::Agent>,
        text: &str,
        root: &std::path::Path,
    ) -> async_channel::Receiver<AgentEvent> {
        let facts = crate::context::RuntimeFacts {
            cwd: root.to_string_lossy().to_string(),
            confined: true,
            sandbox_reason: None,
            permission: "workspace-write".to_string(),
            preset: "standard".to_string(),
            presets_disabled: false,
        };
        runner.spawn(
            id,
            agent,
            react::llm::Content::Text(text.to_string()),
            Vec::new(),
            facts,
            None,
            1_000,
            crate::context_assembly::MemoryState::Disabled,
            false,
            None,
        )
    }

    /// A greeting does not carry an installed extension's workflow body: the
    /// manifest names the omission and its reason (issue #436).
    #[test]
    fn a_greeting_does_not_inject_extension_instructions() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let agent = request_capturing_agent(seen.clone());
        agent.add_conditional_prompt(devkit_prompt());
        let root = workspace("extension-chat");
        let mut runner = Runner::new();
        let receiver = spawn_turn(&mut runner, "extension-chat", agent, "hello 8888", &root);
        drain(receiver);

        let requests = seen.lock().unwrap();
        let request = requests.last().expect("a recorded request");
        let messages = format!("{:?}", request.messages);
        assert!(
            !messages.contains("DEVKIT-BODY"),
            "the body is not a message: {messages}"
        );
        assert!(
            !request
                .wire_system_prompt()
                .is_some_and(|prompt| prompt.contains("DEVKIT-BODY")),
            "the body is not the system prompt"
        );
        assert!(
            request
                .manifest
                .contains("com.example.devkit@1.2.0#guide#prompt"),
            "the manifest names the omitted contribution: {}",
            request.manifest
        );
        assert!(
            request
                .manifest
                .contains("not relevant to this contribution"),
            "the manifest carries the omission reason: {}",
            request.manifest
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A deferred extension capability is absent from the initial array but
    /// still admitted; on a work turn it activates the extension's guidance
    /// (issue #436 review).
    #[test]
    fn an_admitted_deferred_capability_activates_the_extension() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut provider = agent::agent::agentic::LlmProvider::new();
        provider.register_vendor(
            "mock".to_string(),
            Box::new(RequestCapturingLlm(seen.clone())),
        );
        let mut agent = agent::Agent::new(
            agent::AgentConfig {
                model: "mock-model".to_string(),
                ..Default::default()
            },
            Arc::new(provider),
        );
        agent.add_tool(Arc::new(agent::tools::FunctionTool::new(
            "devkit__scaffold",
            "scaffold a package",
            serde_json::json!({"type": "object"}),
            |_args| Ok(serde_json::json!({"ok": true})),
        )));
        // The tool is admitted but withheld from the initial array.
        agent.set_initial_tool_scope(react::tool::admission::InitialToolScope::new(
            ["read_file"],
            Vec::<String>::new(),
        ));
        agent.add_conditional_prompt(devkit_prompt());
        let root = workspace("extension-capability");
        let mut runner = Runner::new();
        let receiver = spawn_turn(
            &mut runner,
            "extension-capability",
            Arc::new(agent),
            "run devkit__scaffold for this build",
            &root,
        );
        drain(receiver);

        let requests = seen.lock().unwrap();
        let messages = format!("{:?}", requests.last().expect("a request").messages);
        assert!(
            messages.contains("DEVKIT-BODY"),
            "the admitted capability activates its guidance: {messages}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Contributions are selected one by one: a task that matches only the
    /// later contribution includes it and reports the first as omitted, with
    /// each contribution's own reason (issue #436 review).
    #[test]
    fn extension_contributions_are_selected_and_reported_individually() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let agent = request_capturing_agent(seen.clone());
        let mut guide = devkit_prompt();
        guide.id = "guide".to_string();
        guide.text = "GUIDE-BODY".to_string();
        guide.triggers = vec!["extension".to_string(), "build".to_string()];
        let mut checklist = devkit_prompt();
        checklist.id = "checklist".to_string();
        checklist.text = "CHECKLIST-BODY".to_string();
        checklist.triggers = vec!["checklist".to_string(), "validate".to_string()];
        agent.add_conditional_prompt(guide);
        agent.add_conditional_prompt(checklist);

        let root = workspace("extension-contributions");
        let mut runner = Runner::new();
        let receiver = spawn_turn(
            &mut runner,
            "extension-contributions",
            agent,
            "validate the checklist",
            &root,
        );
        drain(receiver);

        let requests = seen.lock().unwrap();
        let request = requests.last().expect("a request");
        let messages = format!("{:?}", request.messages);
        assert!(messages.contains("CHECKLIST-BODY"), "{messages}");
        assert!(!messages.contains("GUIDE-BODY"), "{messages}");
        assert!(
            request
                .manifest
                .contains("com.example.devkit@1.2.0#checklist#prompt"),
            "the included contribution is named: {}",
            request.manifest
        );
        assert!(
            request
                .manifest
                .contains("com.example.devkit@1.2.0#guide#prompt"),
            "the omitted contribution is named: {}",
            request.manifest
        );
        assert!(
            request
                .manifest
                .contains("not relevant to this contribution"),
            "the omission carries its reason: {}",
            request.manifest
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The reported regression: an admitted extension tool must not inject
    /// its workflow on an unrelated research turn (issue #436 review).
    #[test]
    fn a_research_turn_does_not_inject_an_admitted_extension() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut provider = agent::agent::agentic::LlmProvider::new();
        provider.register_vendor(
            "mock".to_string(),
            Box::new(RequestCapturingLlm(seen.clone())),
        );
        let mut agent = agent::Agent::new(
            agent::AgentConfig {
                model: "mock-model".to_string(),
                ..Default::default()
            },
            Arc::new(provider),
        );
        agent.add_tool(Arc::new(agent::tools::FunctionTool::new(
            "devkit__scaffold",
            "scaffold a package",
            serde_json::json!({"type": "object"}),
            |_args| Ok(serde_json::json!({"ok": true})),
        )));
        agent.set_initial_tool_scope(react::tool::admission::InitialToolScope::new(
            ["read_file"],
            Vec::<String>::new(),
        ));
        agent.add_conditional_prompt(devkit_prompt());
        let root = workspace("extension-research");
        let mut runner = Runner::new();
        let receiver = spawn_turn(
            &mut runner,
            "extension-research",
            Arc::new(agent),
            "research the latest llm papers",
            &root,
        );
        drain(receiver);

        let requests = seen.lock().unwrap();
        let request = requests.last().expect("a request");
        let messages = format!("{:?}", request.messages);
        assert!(
            !messages.contains("DEVKIT-BODY"),
            "the extension guide is not injected: {messages}"
        );
        assert!(
            request
                .manifest
                .contains("not relevant to this contribution"),
            "the omission is reported: {}",
            request.manifest
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Explicit extension work injects the body once, with the extension's id
    /// and version in the manifest (issue #436).
    #[test]
    fn extension_work_injects_the_body_with_provenance() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let agent = request_capturing_agent(seen.clone());
        agent.add_conditional_prompt(devkit_prompt());
        let root = workspace("extension-work");
        let mut runner = Runner::new();
        let receiver = spawn_turn(
            &mut runner,
            "extension-work",
            agent,
            "build a new extension package",
            &root,
        );
        drain(receiver);

        let requests = seen.lock().unwrap();
        let request = requests.last().expect("a recorded request");
        let messages = format!("{:?}", request.messages);
        assert!(
            messages.contains("DEVKIT-BODY"),
            "the active body rides as a context row: {messages}"
        );
        assert!(
            request
                .manifest
                .contains("com.example.devkit@1.2.0#guide#prompt"),
            "the manifest names the included contribution: {}",
            request.manifest
        );
        assert!(
            request.manifest.contains("\"included\":true"),
            "the included block is marked: {}",
            request.manifest
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// With no instruction file, only the snapshot and the input are sent -
    /// never an empty instructions message.
    #[test]
    fn a_turn_without_instructions_injects_no_empty_message() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let agent = capturing_agent(seen.clone());
        let root = workspace("no-context");
        let facts = crate::context::RuntimeFacts {
            cwd: root.to_string_lossy().to_string(),
            confined: true,
            sandbox_reason: None,
            permission: "workspace-write".to_string(),
            preset: "standard".to_string(),
            presets_disabled: false,
        };
        assert_eq!(
            crate::context::TurnContext::from_facts(&facts, false).instructions,
            None
        );
        let mut runner = Runner::new();
        let receiver = runner.spawn(
            "no-context-turn",
            agent,
            react::llm::Content::Text("go".to_string()),
            Vec::new(),
            facts,
            None,
            1_000,
            crate::context_assembly::MemoryState::Disabled,
            false,
            None,
        );
        drain(receiver);
        let rows = captured_text(&seen);
        assert_eq!(rows.len(), 3, "snapshot, time and input only: {rows:?}");
        assert!(rows[0].starts_with(crate::context::RUNTIME_CONTEXT_PREAMBLE));
        assert!(rows[1].starts_with(crate::context::TIME_CONTEXT_PREAMBLE));
        assert_eq!(rows[2], "go");
        assert!(
            rows.iter().all(|row| !row.is_empty()),
            "no empty injected message: {rows:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A child turn carries the delegation scope in its snapshot, and the
    /// read-only child view does NOT show the harness's own injection as a
    /// conversation row.
    #[test]
    fn a_child_turn_carries_the_delegation_context() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let child = capturing_agent(seen.clone());
        let root = workspace("child-context");
        let facts = crate::context::RuntimeFacts {
            cwd: root.to_string_lossy().to_string(),
            confined: true,
            sandbox_reason: None,
            permission: "workspace-write".to_string(),
            preset: "standard".to_string(),
            presets_disabled: false,
        };
        let context = crate::context::TurnContext::from_facts(&facts, true);
        let registry = Arc::new(crate::subagent::SubagentRegistry::default());
        let result = futures::executor::block_on(crate::caps::stream_child(
            &child,
            "do the task".to_string(),
            registry,
            "child-run",
            &context,
        ));
        assert!(result.is_ok(), "the child turn runs: {result:?}");
        let rows = captured_text(&seen);
        assert_eq!(rows.len(), 3, "snapshot, time and input: {rows:?}");
        assert!(rows[0].starts_with(crate::context::RUNTIME_CONTEXT_PREAMBLE));
        assert!(
            rows[0].contains(crate::context::SUBAGENT_CONTEXT),
            "the child snapshot carries the delegation scope: {}",
            rows[0]
        );
        assert!(rows[0].contains("Permission: workspace-write."));
        assert!(rows[1].starts_with(crate::context::TIME_CONTEXT_PREAMBLE));
        assert_eq!(rows[2], "do the task");

        // The live child transcript is what the pane draws; the injected
        // snapshot is not a row of the child's own conversation.
        let transcript = crate::caps::child_transcript(&child);
        assert!(
            transcript.iter().all(|row| !row
                .content
                .starts_with(crate::context::RUNTIME_CONTEXT_PREAMBLE)),
            "the injected snapshot is not a child row: {transcript:?}"
        );
        assert!(
            transcript.iter().any(|row| row.content == "do the task"),
            "the child's own prompt is still shown: {transcript:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The stored tool traffic replays into the next turn's session.
    ///
    /// Dropping it used to make every turn re-do the last turn's reads - the
    /// model re-read the same files each turn because the results it already
    /// held were gone - and it moved the request prefix between turns. The
    /// replay pairs each call with its result under a seed-local id.
    #[test]
    fn seeding_replays_tool_calls_with_their_results() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let agent = capturing_agent(seen);
        let mut assistant = crate::model::ChatMessage::assistant();
        assistant.text = "reading".to_string();
        assistant.tools = vec![
            crate::model::ToolEvent {
                name: "read_file".to_string(),
                args: "{\"path\":\"src/main.rs\"}".to_string(),
                output: Some("fn main() {}".to_string()),
                ms: Some(3),
                at: None,
            },
            crate::model::ToolEvent {
                name: "grep".to_string(),
                args: "{\"pattern\":\"fn\"}".to_string(),
                output: Some("src/main.rs:1:fn main".to_string()),
                ms: Some(1),
                at: None,
            },
        ];
        let transcript = vec![
            crate::model::ChatMessage::user("look at main".to_string()),
            assistant,
        ];
        Runner::seed_session(&agent, &transcript);
        let session = agent.session();
        let messages = session.messages();
        let mut calls = 0usize;
        let mut results = 0usize;
        for (index, message) in messages.iter().enumerate() {
            match message {
                react::LlmMessage::AssistantToolCall {
                    tool_call_id,
                    name,
                    args,
                } => {
                    calls += 1;
                    let expected_name = if calls == 1 { "read_file" } else { "grep" };
                    assert_eq!(name, expected_name);
                    if calls == 1 {
                        assert_eq!(args["path"], "src/main.rs");
                    }
                    match messages.get(index + 1) {
                        Some(react::LlmMessage::ToolResult {
                            tool_call_id: result_id,
                            content,
                        }) => {
                            assert_eq!(result_id, tool_call_id, "the pair shares its id");
                            assert!(
                                content.contains("fn main"),
                                "the result is the stored output: {content}"
                            );
                        }
                        other => panic!("a call is followed by its result: {other:?}"),
                    }
                }
                react::LlmMessage::ToolResult { .. } => results += 1,
                _ => {}
            }
        }
        assert_eq!(
            (calls, results),
            (2, 2),
            "both calls replay with their results: {messages:?}"
        );
    }

    /// Reasoning stays in the transcript but never reaches a later turn.
    ///
    /// A model must not receive a previous turn's chain of thought or its
    /// opaque provider state; only the engine's live session for the current
    /// turn carries those.
    #[test]
    fn seeding_never_replays_a_previous_turns_reasoning() {
        let agent = capturing_agent(Arc::new(std::sync::Mutex::new(Vec::new())));
        let mut assistant = crate::model::ChatMessage::assistant();
        assistant.reasoning = "private thought".to_string();
        assistant.parts = vec![crate::model::TurnPart::Thinking(15)];
        assistant.text = "answer".to_string();
        Runner::seed_session(
            &agent,
            &[
                crate::model::ChatMessage::user("solve it".to_string()),
                assistant,
            ],
        );
        let session = agent.session();
        assert!(
            !session.messages().iter().any(|message| matches!(
                message,
                react::llm::LlmMessage::AssistantReasoning { .. }
            )),
            "a later turn seeds no reasoning"
        );
        assert!(
            session.messages().iter().any(|message| matches!(
                message,
                react::llm::LlmMessage::Assistant { content } if content == "answer"
            )),
            "the visible answer still replays"
        );
    }

    /// The complete turn-local reasoning lifecycle across a real BSH turn:
    /// two reasoning blocks with distinct provider state survive a two-step
    /// tool loop in order, then the next turn is seeded from the transcript
    /// and carries neither block nor either state. This drives the production
    /// seed path and the engine request builder, not a helper in isolation.
    #[test]
    fn a_bsh_turn_replays_its_reasoning_and_the_next_turn_drops_it() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut provider = agent::agent::agentic::LlmProvider::new();
        provider.register_vendor(
            "mock".to_string(),
            Box::new(ReasoningLifecycleLlm {
                seen: Arc::clone(&seen),
                step: std::sync::atomic::AtomicUsize::new(0),
            }),
        );
        let mut agent = agent::Agent::new(
            agent::AgentConfig {
                model: "mock-model".to_string(),
                ..Default::default()
            },
            Arc::new(provider),
        );
        agent.add_tool(Arc::new(agent::tools::FunctionTool::new(
            "echo",
            "echo the argument",
            serde_json::json!({
                "type": "object",
                "properties": { "n": { "type": "integer" } }
            }),
            |args| Ok(serde_json::json!({ "echoed": args.get("n").cloned().unwrap_or_default() })),
        )));
        let agent = Arc::new(agent);

        let root = workspace("reasoning-lifecycle");
        let mut runner = Runner::new();
        let receiver = spawn_turn(
            &mut runner,
            "reasoning-lifecycle",
            Arc::clone(&agent),
            "go",
            &root,
        );
        drain(receiver);

        // The transcript the reader keeps: visible answer, reasoning for the
        // disclosure, and the two tool exchanges. Reasoning is display-only.
        let mut assistant = crate::model::ChatMessage::assistant();
        assistant.text = "finished".to_string();
        assistant.reasoning = "alpha beta".to_string();
        assistant.tools = vec![
            crate::model::ToolEvent {
                name: "echo".to_string(),
                args: r#"{"n":1}"#.to_string(),
                output: Some(r#"{"echoed":1}"#.to_string()),
                ms: Some(1),
                at: None,
            },
            crate::model::ToolEvent {
                name: "echo".to_string(),
                args: r#"{"n":2}"#.to_string(),
                output: Some(r#"{"echoed":2}"#.to_string()),
                ms: Some(1),
                at: None,
            },
        ];
        let transcript = vec![crate::model::ChatMessage::user("go"), assistant];
        let facts = crate::context::RuntimeFacts {
            cwd: root.to_string_lossy().to_string(),
            confined: true,
            sandbox_reason: None,
            permission: "workspace-write".to_string(),
            preset: "standard".to_string(),
            presets_disabled: false,
        };
        let receiver = runner.spawn(
            "reasoning-lifecycle",
            agent,
            react::llm::Content::Text("next".to_string()),
            transcript,
            facts,
            None,
            1_000,
            crate::context_assembly::MemoryState::Disabled,
            false,
            None,
        );
        drain(receiver);
        let _ = std::fs::remove_dir_all(&root);

        let requests = seen.lock().unwrap();
        assert_eq!(
            requests.len(),
            4,
            "two tool steps, the answer, then the reseeded turn: {}",
            requests.len()
        );
        assert!(
            reasoning_blocks(&requests[0].messages).is_empty(),
            "the first step has no reasoning to replay"
        );
        assert_eq!(
            reasoning_blocks(&requests[1].messages),
            vec![("alpha".to_string(), Some(lifecycle_state("sig-alpha")),)],
            "the second request replays block A with its own state"
        );
        assert_eq!(
            reasoning_blocks(&requests[2].messages),
            vec![
                ("alpha".to_string(), Some(lifecycle_state("sig-alpha"))),
                ("beta".to_string(), Some(lifecycle_state("sig-beta"))),
            ],
            "the third request replays A then B, each with its matching state"
        );

        let messages = &requests[2].messages;
        let alpha = messages
            .iter()
            .position(|message| {
                matches!(
                    message,
                    react::llm::LlmMessage::AssistantReasoning { content, .. }
                        if content == "alpha"
                )
            })
            .expect("block A is in the request");
        let first_call = messages
            .iter()
            .position(|message| {
                matches!(
                    message,
                    react::llm::LlmMessage::AssistantToolCall { name, args, .. }
                        if name == "echo" && args["n"] == 1
                )
            })
            .expect("the first call is in the request");
        let first_result = messages
            .iter()
            .position(|message| matches!(message, react::llm::LlmMessage::ToolResult { .. }))
            .expect("the first result is in the request");
        let beta = messages
            .iter()
            .position(|message| {
                matches!(
                    message,
                    react::llm::LlmMessage::AssistantReasoning { content, .. }
                        if content == "beta"
                )
            })
            .expect("block B is in the request");
        let second_call = messages
            .iter()
            .position(|message| {
                matches!(
                    message,
                    react::llm::LlmMessage::AssistantToolCall { name, args, .. }
                        if name == "echo" && args["n"] == 2
                )
            })
            .expect("the second call is in the request");
        assert!(
            alpha < first_call
                && first_call < first_result
                && first_result < beta
                && beta < second_call,
            "the blocks keep the order the turn produced them: {messages:?}"
        );

        let turn_two = &requests[3].messages;
        assert!(
            reasoning_blocks(turn_two).is_empty(),
            "the reseeded turn begins with no previous-turn reasoning: {turn_two:?}"
        );
        let turn_two_text = format!("{turn_two:?}");
        for private in ["alpha", "beta", "sig-alpha", "sig-beta"] {
            assert!(
                !turn_two_text.contains(private),
                "the reseeded turn carries no {private}: {turn_two_text}"
            );
        }
        assert!(
            turn_two.iter().any(|message| matches!(
                message,
                react::llm::LlmMessage::Assistant { content } if content == "finished"
            )),
            "the visible answer still replays from the transcript"
        );
        assert_eq!(
            turn_two
                .iter()
                .filter(|message| matches!(
                    message,
                    react::llm::LlmMessage::AssistantToolCall { name, .. } if name == "echo"
                ))
                .count(),
            2,
            "the turn's tool traffic still replays"
        );
    }

    /// A stored `/command` annotation replays as the block its live send
    /// attached, so a later turn's model does not read a name it has no
    /// instruction for. The stored row itself stays the reader's line.
    #[test]
    fn a_stored_command_annotation_replays_as_its_block() {
        let commands = vec![crate::extension::CommandSurface {
            name: "hello__greet".to_string(),
            description: "Greet the reader".to_string(),
            template: "Say hello from com.example.hello.".to_string(),
        }];
        let agent = capturing_agent(Arc::new(std::sync::Mutex::new(Vec::new())));
        let transcript = vec![
            crate::model::ChatMessage::user("/hello__greet in one sentence".to_string()),
            crate::model::ChatMessage::assistant(),
        ];
        Runner::seed_session_with(&agent, &transcript, &commands);
        let users: Vec<String> = agent
            .session()
            .messages()
            .iter()
            .filter_map(|message| match message {
                react::llm::LlmMessage::User { content } => {
                    Some(content.as_text().unwrap_or("").to_string())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            users,
            vec![
                "<extension_command name=\"hello__greet\">\nSay hello from com.example.hello.\n</extension_command>\n\n---\n\nin one sentence"
                    .to_string()
            ],
            "the replayed row carries the block again"
        );
        assert_eq!(
            transcript[0].text, "/hello__greet in one sentence",
            "seeding never rewrites the stored transcript"
        );
    }

    /// The injected frame sits at the HEAD of a turn that carries history.
    ///
    /// Appending it behind the history moved the frame one turn's worth every
    /// time, which diverged the request prefix at the second row and kept the
    /// provider's prompt cache from ever carrying across turns.
    #[test]
    fn the_frame_precedes_the_replayed_history() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let agent = capturing_agent(seen.clone());
        let root = workspace("frame-head");
        let facts = crate::context::RuntimeFacts {
            cwd: root.to_string_lossy().to_string(),
            confined: true,
            sandbox_reason: None,
            permission: "workspace-write".to_string(),
            preset: "standard".to_string(),
            presets_disabled: false,
        };
        let transcript = vec![crate::model::ChatMessage::user("earlier line".to_string())];
        let mut runner = Runner::new();
        let receiver = runner.spawn(
            "frame-turn",
            agent,
            react::llm::Content::Text("now".to_string()),
            transcript,
            facts,
            None,
            1_000,
            crate::context_assembly::MemoryState::Disabled,
            false,
            None,
        );
        drain(receiver);
        let rows = captured_text(&seen);
        assert_eq!(rows.len(), 4, "snapshot, time, history, input: {rows:?}");
        assert!(
            rows[0].starts_with(crate::context::RUNTIME_CONTEXT_PREAMBLE),
            "the frame is the first message: {rows:?}"
        );
        assert!(rows[1].starts_with(crate::context::TIME_CONTEXT_PREAMBLE));
        assert_eq!(rows[2], "earlier line", "the replayed history follows it");
        assert_eq!(rows[3], "now", "the reader's line comes last");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A child follow-up (send_message / the UI reopen) runs stream_child over
    /// the SAME live session. The session-scoped AGENTS.md block and the
    /// identical snapshot already sit in that session's log, so the second
    /// turn must not inject them again; only the fresh time-context is new.
    #[test]
    fn a_child_follow_up_keeps_one_instructions_block() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let child = capturing_agent(seen.clone());
        let root = workspace("child-dedup");
        std::fs::write(root.join("AGENTS.md"), "workspace rule").expect("the file");
        let facts = crate::context::RuntimeFacts {
            cwd: root.to_string_lossy().to_string(),
            confined: true,
            sandbox_reason: None,
            permission: "workspace-write".to_string(),
            preset: "standard".to_string(),
            presets_disabled: false,
        };
        let registry = Arc::new(crate::subagent::SubagentRegistry::default());
        let context = crate::context::TurnContext::from_facts(&facts, true);
        let first = futures::executor::block_on(crate::caps::stream_child(
            &child,
            "first task".to_string(),
            registry.clone(),
            "child-run",
            &context,
        ));
        assert!(first.is_ok(), "the first child turn runs: {first:?}");
        let follow = crate::context::TurnContext::from_facts(&facts, true);
        let second = futures::executor::block_on(crate::caps::stream_child(
            &child,
            "second task".to_string(),
            registry,
            "child-run",
            &follow,
        ));
        assert!(second.is_ok(), "the follow-up runs: {second:?}");
        let rows = captured_text(&seen);
        assert_eq!(
            rows.iter()
                .filter(|row| row.contains("Instructions from: AGENTS.md"))
                .count(),
            1,
            "the follow-up does not re-inject the session's AGENTS.md: {rows:?}"
        );
        assert_eq!(
            rows.iter()
                .filter(|row| row.starts_with(crate::context::RUNTIME_CONTEXT_PREAMBLE))
                .count(),
            1,
            "the identical snapshot is not stacked: {rows:?}"
        );
        assert!(
            rows.iter().any(|row| row == "first task"),
            "first prompt kept: {rows:?}"
        );
        assert!(
            rows.iter().any(|row| row == "second task"),
            "follow-up prompt present: {rows:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A mock that holds the stream open, so a stop lands before Done.
    struct SlowMock;

    #[async_trait::async_trait]
    impl agent::LlmClient<agent::agent::AgentSession, agent::agent::AgentReactContext> for SlowMock {
        async fn complete(
            &self,
            _persona: Option<String>,
            _req: react::llm::LlmRequest,
            _session: &mut agent::agent::AgentSession,
            _context: &mut agent::agent::AgentReactContext,
        ) -> react::llm::LlmResponseResult {
            Ok(summary_response("late"))
        }

        async fn stream_complete(
            &self,
            _persona: Option<String>,
            _req: react::llm::LlmRequest,
            _session: &mut agent::agent::AgentSession,
            _context: &mut agent::agent::AgentReactContext,
        ) -> Result<react::llm::TokenStream, react::llm::LlmError> {
            Ok(Box::pin(futures::stream::unfold(0u8, |state| async move {
                if state == 0 {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    Some((Ok(react::llm::StreamToken::Done), 1))
                } else {
                    None
                }
            })))
        }

        fn provider_name(&self) -> &'static str {
            "slow-mock"
        }
    }

    fn capture_agent(
        vendor: &str,
        client: Box<
            dyn agent::LlmClient<agent::agent::AgentSession, agent::agent::AgentReactContext>,
        >,
    ) -> Arc<agent::agent::Agent> {
        let mut provider = agent::agent::agentic::LlmProvider::new();
        provider.register_vendor(vendor.to_string(), client);
        Arc::new(agent::agent::Agent::new(
            agent::agent::AgentConfig {
                model: "mock-model".to_string(),
                ..Default::default()
            },
            Arc::new(provider),
        ))
    }

    fn test_capture(
        dir: &std::path::Path,
        rt: &tokio::runtime::Runtime,
    ) -> Arc<crate::memory_capture::CaptureHandle> {
        let store = rt
            .block_on(agent::memory::FileMemory::open_scoped(
                dir.join("ws.jsonl"),
                agent::memory::MemoryScope::workspace("w"),
                Some(agent::memory::MemoryScope::session("w", "s")),
            ))
            .unwrap();
        let policy = agent::memory::capture::CapturePolicy {
            enabled: true,
            scope: agent::memory::MemoryScope::session("w", "s"),
            ..Default::default()
        };
        crate::memory_capture::CaptureHandle::new(Arc::new(store), policy)
    }

    fn test_facts() -> crate::context::RuntimeFacts {
        crate::context::RuntimeFacts {
            cwd: String::new(),
            confined: true,
            sandbox_reason: None,
            permission: "workspace-write".to_string(),
            preset: "standard".to_string(),
            presets_disabled: false,
        }
    }

    /// Issue #343: capture runs at the COMPLETED-turn boundary only. A Done
    /// turn offers its evidence asynchronously; a stopped turn offers nothing.
    #[test]
    fn capture_runs_on_done_turns_and_not_on_stopped_turns() {
        let dir = tempfile::tempdir().unwrap();
        let mut runner = Runner::new();
        let capture = test_capture(dir.path(), runner.rt());

        // A completed turn: the explicit request is captured beside the turn.
        let agent = capture_agent("mock", Box::new(SummaryMock));
        let rx = runner.spawn(
            "capture-done",
            agent,
            react::llm::Content::Text("Remember this: the gate runs clippy".to_string()),
            Vec::new(),
            test_facts(),
            None,
            1_000,
            crate::context_assembly::MemoryState::Disabled,
            false,
            Some(Arc::clone(&capture)),
        );
        futures::executor::block_on(async {
            while let Ok(event) = rx.recv().await {
                if matches!(event, AgentEvent::Done | AgentEvent::Error(_)) {
                    break;
                }
            }
        });
        runner.rt().block_on(async {
            for _ in 0..400 {
                if capture.last_summary().is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });
        let summary = capture.last_summary().expect("the Done turn captured");
        assert_eq!(summary.committed, 1, "the explicit request was committed");

        // A stopped turn: no Done, no capture.
        let stopped = test_capture(dir.path(), runner.rt());
        let slow = capture_agent("slow", Box::new(SlowMock));
        let rx = runner.spawn(
            "capture-stopped",
            slow,
            react::llm::Content::Text("Remember this: never captured".to_string()),
            Vec::new(),
            test_facts(),
            None,
            1_000,
            crate::context_assembly::MemoryState::Disabled,
            false,
            Some(Arc::clone(&stopped)),
        );
        runner.stop("capture-stopped");
        futures::executor::block_on(async {
            while let Ok(event) = rx.recv().await {
                if matches!(event, AgentEvent::Done | AgentEvent::Error(_)) {
                    break;
                }
            }
        });
        runner.rt().block_on(async {
            tokio::time::sleep(Duration::from_millis(100)).await;
        });
        assert!(
            stopped.last_summary().is_none(),
            "a stopped turn must not run capture"
        );
    }
}
