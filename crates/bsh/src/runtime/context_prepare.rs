//! One turn's context assembly, shared by both interfaces.
//!
//! A desktop turn and a terminal turn read the same runtime facts, the same
//! working plan, the same workspace instructions, the same memory recall and
//! the same extension prompts, or the two surfaces disagree about what the
//! model was told. The assembly therefore lives here: the runner and the Host
//! executor both call [prepare], and the blocks it injects come from one typed
//! snapshot rather than a separately rendered copy.
//!
//! The call is one pipeline: it builds the snapshot, injects it at the
//! head of the request, records the provider-request manifest, and reports the
//! same diagnostics a reader can inspect. The display-only injection rows are
//! returned so an interface that draws them (the window) can; an interface that
//! does not (the terminal) simply drops them - the request is identical.

use agent::Agent;
use react::llm::Content;

use crate::context::{RuntimeFacts, TimeFacts};
use crate::context_assembly::MemoryState;
use crate::model::ContextInjection;

/// The live plan block for one turn, when the agent has a plan.
///
/// Built from the agent's own store (the `update_plan` tool writes through the
/// same one), so the block is the plan the panel shows and not a second copy.
pub(crate) fn plan_context_block(agent: &Agent) -> Option<crate::context_assembly::ContextBlock> {
    let plan = crate::context::render_plan(&agent.plan_items())?;
    Some(crate::context_assembly::ContextBlock {
        source: crate::context_assembly::ContextSource::Plan,
        trust: crate::context_assembly::ContextSource::Plan.trust(),
        token_estimate: agent::agent::context::estimate_tokens(&plan),
        provenance: Some("plan".to_string()),
        text: plan,
    })
}

/// Build, inject and report one turn's context.
///
/// - agent: this turn's build, whose own prompt assembly supplies the persona.
/// - input: the request being sent, after the turn's skill expansion.
/// - facts: the one resolution the build used.
/// - time: the turn number and the span since the previous preparation.
/// - memory_state: what the build resolved for memory.
/// - persona_complete: whether the configuration asked for a complete persona
///   (diagnostics only; the harness layers are always composed).
/// - session: the session the turn belongs to, for the snapshot version.
/// - transcript_len: the rows the turn replays, for the snapshot version.
///
/// Returns the display-only injection rows. The provider request itself has
/// already been injected into `agent` when this returns.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn prepare(
    agent: &Agent,
    input: &Content,
    facts: &RuntimeFacts,
    time: &TimeFacts,
    memory_state: MemoryState,
    persona_complete: bool,
    session: &str,
    transcript_len: usize,
) -> Vec<ContextInjection> {
    // The turn's own context is rendered HERE, on the caller's blocking
    // path: the snapshot is pure, but instruction discovery stats the
    // workspace, and a send path must not do file I/O on the UI thread.
    // It is injected after the checkpoint so the fresh snapshot cannot be
    // summarized into the history it supersedes.
    let context = crate::context::TurnContext::from_facts_at(facts, false, time);
    // The typed snapshot is built below, from the same facts, and its
    // request blocks are what get injected: no separately rendered
    // copy can drift from the assembled types (issue #411).
    // The display-only rows: the persona the build composed, the runtime
    // snapshot and the workspace instructions this turn carried. A caller
    // that draws a transcript records them; the request itself does not
    // depend on that.
    {
        let mut injections: Vec<crate::model::ContextInjection> = Vec::new();
        // The TYPED assembly (issue #346): the exact system prompt plus
        // the recall account it was built from. Diagnostics read the
        // account, never search the rendered text.
        let assembly = agent.assemble_prompt(input).await;
        // Retrieved memory is not part of the system prompt: it rides
        // the provider request's untrusted context channel.
        let persona = assembly.base_prompt.clone();
        let recall = crate::context_assembly::MemoryRecallInfo::from_agent(&assembly.memory);

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
        // The working plan is host state the model maintains, so it
        // rides the request like the runtime facts: after a compaction
        // or a long turn the old tool row may be all the model has.
        if let Some(block) = plan_context_block(agent) {
            blocks.push(block);
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
                    source: format!("{}@{}#{}", active.owner, active.version, prompt.id),
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
        let memory_revision = (!recall.records.is_empty())
            .then(|| crate::context_assembly::digest_text(&recall.records.join(",")));
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
            session: Some(session.to_string()),
            session_state: Some(format!("messages:{}", transcript_len)),
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
        crate::context::inject_snapshot(agent, &snapshot);
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
                report.estimated_tokens = report.estimated_tokens.saturating_add(bytes / 4);
                report.sources.push(crate::context_assembly::SourceStatus {
                    source: crate::context_assembly::ContextSource::Extension,
                    included: true,
                    reason: Some(format!(
                        "{}@{}#{}: {}",
                        active.owner, active.version, selected.prompt.id, selected.reason
                    )),
                    tokens: bytes / 4,
                });
            }
        }
        for omission in snapshot.omissions() {
            report.sources.push(crate::context_assembly::SourceStatus {
                source: crate::context_assembly::ContextSource::Extension,
                included: false,
                reason: Some(format!("{}: {}", omission.provenance, omission.reason)),
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
        if let Some(block) = snapshot.block(crate::context_assembly::ContextSource::RetrievedMemory)
        {
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
            let source = crate::instructions::sources(std::path::Path::new(&facts.cwd)).join(", ");
            injections.push(crate::model::ContextInjection {
                kind: "instructions".to_string(),
                content: instructions.clone(),
                source,
            });
        }
        injections
    }
}
