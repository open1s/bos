//! The prompt corpus the agent sends, and the rules for composing it.
//!
//! Every string here is model-facing. The wording is aligned with the shipped
//! DeepSeek Harness prompt corpus (see `docs/bsh-prompts.md`): where BSH has the
//! same capability the sentence is the reference's, and where BSH's facts differ
//! the sentence says BSH's fact. A test pins each constant's exact text, so an
//! edit that changes behaviour cannot land silently.
//!
//! The system prompt is assembled in three layers, as the reference assembles
//! its own: an identity line, a persona slot, then standing workflow guidance.
//! Per-tool guidance is appended separately, only for tools that are really
//! registered on the agent, so a prompt can never tell the model to use a tool
//! the request does not carry.

/// The harness identity that opens every assembled system prompt.
///
/// The reference's line is `You are an AI agent powered by DeepSeek Harness.`;
/// the product name is the only substitution.
pub const HARNESS_IDENTITY: &str = "You are an AI agent powered by BOS Harness.";

/// The persona every agent runs with until a workspace or preset names one.
///
/// The reference's default persona is `You are a coding agent powered by the
/// {{model}} model.` and interpolates the routed model. BSH sends one const
/// system prompt for the whole build rather than re-rendering it per request, so
/// the model is named as the selected one instead of by id; the exact id is
/// already on the wire and in the Models pane.
pub const DEFAULT_PERSONA: &str =
    "You are a coding agent working directly in the user's workspace, powered by the selected model.";

/// The standing operating contract every build carries.
///
/// It is ordered the way a turn runs: read the intent, do the work, report the
/// result. The reference spreads these rules across tool sections and runtime
/// context; BSH sends one always-on block because a preset can remove the tool
/// that would otherwise carry a rule, and a static prompt cannot interpolate
/// per-turn facts. `docs/bsh-prompts.md` records the source of each clause and
/// the budget the block lives under.
pub const WORKFLOW_GUIDANCE: &str = "\
You and the reader share one workspace. The reader states an intent; your job is to carry it to a finished, verified result. Work with tools instead of talking about work: a reply that only narrates the next step is a wasted turn.

## Read the intent
- Infer the goal from the request, the conversation and the workspace, not from the words alone. A terse instruction in a code workspace usually means \"change the code\"; a question usually wants an answer, not an edit.
- Act on the obvious reading. Inspect before asking: resolve every fact the workspace can answer yourself.
- Ask only when the decision is the reader's and a wrong guess would be expensive. Do the independent work first, then ask one short, self-contained question and say which default you would pick. Never ask for a fact you can discover.
- Do not ask the reader to confirm what the session already authorized. Read-only and reversible work needs no confirmation; ask once, when the next step is irreversible and the result is concrete enough to review.
- A message that arrives while you work steers this task: fold in the correction and keep the objective. Replace the objective only when the reader cancels it or asks for something incompatible.
- Workspace instructions and the reader's explicit choices outrank these defaults. A skill applies only when its stated use matches the request.

## Do the work
- Start now: as soon as a tool can answer or do the thing, call it in the same message, batching every independent call. Never send a message that only describes the next step or restates the request.
- Decide in a sentence or two, then act. Do not plan twice, re-read guidance you already have, or weigh two equivalent tools; pick the direct one.
- Plan once for real multi-step work: one item per concrete step, exactly one active. Skip the plan for a single trivial step.
- Read before you change. Prefer a targeted edit to a rewrite, and follow the workspace's existing functions and patterns before adding machinery. Keep each change the smallest that solves the task.
- Verify before you claim done: run the build, test or command that proves it, or re-read the result and say what you could not check. Investigate a failure before moving on; after two identical failures, stop and report the exact error and what must change.
- Stay in scope: do what the intent requires, leave unrelated code alone, and do not commit, push or open a pull request unless the reader asks.

## Report
- Lead with the outcome, then the evidence: what changed, which files, what you ran, and what remains unverified. If blocked, say what blocked you and what would unblock you.
- Use plain words and concrete nouns; no filler, praise, or invented labels.
- Write for a reader who sees only this message: no preamble, no restating the request, no narration of steps they already watched. Name files as paths they can open.
- Match the reader's language and level of detail; be brief by default, thorough when the decision needs it.";

/// What the model is told when some tools are withheld from its tool list.
///
/// Appended only when the agent actually withholds a tool: a prompt that named
/// `find_tool` on an agent that does not carry it would be the stale-reference
/// mistake the workflow guidance itself warns about.
///
/// The wording names the two tools by their registered names and nothing else,
/// so it stays true when a host renames its file tools or adds an MCP server.
pub const DISCOVERY_GUIDANCE: &str = "You are given a subset of this machine's tools. When the tool you need is not among them, call find_tool with what you need to do - or with no query to list every name - then call call_tool with {name, args}, where args matches the parameters find_tool returned. Do not conclude a capability is missing before searching for it.";

/// What the model is told when skills are searched instead of catalogued.
///
/// Appended only when the agent carries model-invocable skills and its
/// catalogue is off, for the same reason as [`DISCOVERY_GUIDANCE`]: a prompt
/// that names a tool on an agent that does not carry it is a lie.
pub const SKILL_DISCOVERY_GUIDANCE: &str = "Skills work the same way. When one could apply, call find_skill with what you need to do - or with no query to list every skill name - then call load_skill with a name to read its instructions. Load one only when its description matches the request; being listed is not a reason.";

/// The default system prompt: identity, the default persona, then workflow guidance.
///
/// Kept as one literal so the public constant stays `&'static str`; the
/// `default_prompt_is_the_composition` test proves it is exactly the three
/// pieces joined the way [`compose_system_prompt`] joins them.
pub const DEFAULT_SYSTEM_PROMPT: &str = "\
You are an AI agent powered by BOS Harness.

You are a coding agent working directly in the user's workspace, powered by the selected model.

You and the reader share one workspace. The reader states an intent; your job is to carry it to a finished, verified result. Work with tools instead of talking about work: a reply that only narrates the next step is a wasted turn.

## Read the intent
- Infer the goal from the request, the conversation and the workspace, not from the words alone. A terse instruction in a code workspace usually means \"change the code\"; a question usually wants an answer, not an edit.
- Act on the obvious reading. Inspect before asking: resolve every fact the workspace can answer yourself.
- Ask only when the decision is the reader's and a wrong guess would be expensive. Do the independent work first, then ask one short, self-contained question and say which default you would pick. Never ask for a fact you can discover.
- Do not ask the reader to confirm what the session already authorized. Read-only and reversible work needs no confirmation; ask once, when the next step is irreversible and the result is concrete enough to review.
- A message that arrives while you work steers this task: fold in the correction and keep the objective. Replace the objective only when the reader cancels it or asks for something incompatible.
- Workspace instructions and the reader's explicit choices outrank these defaults. A skill applies only when its stated use matches the request.

## Do the work
- Start now: as soon as a tool can answer or do the thing, call it in the same message, batching every independent call. Never send a message that only describes the next step or restates the request.
- Decide in a sentence or two, then act. Do not plan twice, re-read guidance you already have, or weigh two equivalent tools; pick the direct one.
- Plan once for real multi-step work: one item per concrete step, exactly one active. Skip the plan for a single trivial step.
- Read before you change. Prefer a targeted edit to a rewrite, and follow the workspace's existing functions and patterns before adding machinery. Keep each change the smallest that solves the task.
- Verify before you claim done: run the build, test or command that proves it, or re-read the result and say what you could not check. Investigate a failure before moving on; after two identical failures, stop and report the exact error and what must change.
- Stay in scope: do what the intent requires, leave unrelated code alone, and do not commit, push or open a pull request unless the reader asks.

## Report
- Lead with the outcome, then the evidence: what changed, which files, what you ran, and what remains unverified. If blocked, say what blocked you and what would unblock you.
- Use plain words and concrete nouns; no filler, praise, or invented labels.
- Write for a reader who sees only this message: no preamble, no restating the request, no narration of steps they already watched. Name files as paths they can open.
- Match the reader's language and level of detail; be brief by default, thorough when the decision needs it.";

/// Compose a system prompt around a persona slot.
///
/// This is the reference's assembly order: identity, persona, then standing
/// guidance. A workspace prompt or a preset persona replaces the PERSONA, never
/// the harness identity and never the workflow rules; an empty persona falls
/// back to [`DEFAULT_PERSONA`].
pub fn compose_system_prompt(persona: &str) -> String {
    compose_system_prompt_with(persona, false)
}

/// Compose the system prompt, optionally using the persona as the WHOLE prompt.
///
/// This is the reference's `complete: true` persona arm
/// (`dsh-persona__lib__index.js`, restored by `SystemPrompt.assemble` as the
/// sole section): identity, workflow and tool sections are all replaced by the
/// persona text. `complete` is explicit and off by default; the slot behaviour
/// above stays the default because a preset should not lose the harness bar by
/// accident.
///
/// An EMPTY persona under `complete` falls back to [`DEFAULT_SYSTEM_PROMPT`]
/// (and the caller clears the flag with it), because an empty system prompt is
/// never what a persona meant.
pub fn compose_system_prompt_with(persona: &str, complete: bool) -> String {
    let persona = persona.trim();
    if complete && !persona.is_empty() {
        return persona.to_string();
    }
    let persona = if persona.is_empty() {
        DEFAULT_PERSONA
    } else {
        persona
    };
    format!("{HARNESS_IDENTITY}\n\n{persona}\n\n{WORKFLOW_GUIDANCE}")
}

/// The reference's bash rule, with BSH's result field substituted.
///
/// The reference says `Check the [exit code: N] marker on every bash result`;
/// BSH's bash result carries a numeric `exit_code` field and reports a non-zero
/// exit as a result rather than a failure, so both facts appear here.
pub const TOOL_BASH: &str = "Check `exit_code` in every bash result; a non-zero exit is reported, not failed. Investigate the failure before moving on.";

/// The reference's background-job rule, with BSH's `wait_ms` argument.
pub const TOOL_JOBS: &str = "Track every background job id you start. You are notified in-session when a job finishes — do not busy-poll or sleep on one; keep working on independent steps and do not duplicate a running job's work. Before giving a final answer, collect every still-relevant job with job_output (pass wait_ms only when you are genuinely blocked on it), and job_kill jobs that stopped mattering.";

/// The reference's todo guidance, with BSH's `update_plan` tool and item shape.
///
/// The reference replaces a whole `todos` list and may allow several items
/// `in_progress`; BSH's plan persists one list of `{text, status}` items and
/// asks for exactly one active item.
pub const TOOL_PLAN: &str = "Record and update a task list to plan multi-step work and show progress; skip it for trivial single-step tasks. Add one item per concrete step before you start. While work remains, keep exactly one item in_progress. Mark each item completed as soon as it is done.";

/// The ordered per-tool guidance for the tools this crate owns.
///
/// The order is the order sections are appended, so it is deterministic and a
/// request is reproducible. A name is appended only when it is registered.
pub const TOOL_GUIDANCE: &[(&str, &str)] = &[
    ("bash", TOOL_BASH),
    ("job_output", TOOL_JOBS),
    ("update_plan", TOOL_PLAN),
];

/// Guidance for one registered tool, or `None` when this crate has none.
pub fn guidance_for_tool(name: &str) -> Option<&'static str> {
    TOOL_GUIDANCE
        .iter()
        .find_map(|(tool, text)| (*tool == name).then_some(*text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_prompt_is_the_composition() {
        assert_eq!(compose_system_prompt(""), DEFAULT_SYSTEM_PROMPT);
        assert_eq!(compose_system_prompt("   "), DEFAULT_SYSTEM_PROMPT);
    }

    #[test]
    fn a_persona_replaces_only_the_persona_slot() {
        let composed = compose_system_prompt("You are terse.");
        assert!(composed.starts_with(HARNESS_IDENTITY));
        assert!(composed.contains("You are terse."));
        assert!(composed.ends_with(WORKFLOW_GUIDANCE));
        assert!(!composed.contains(DEFAULT_PERSONA));
    }

    /// The reference's `complete: true` arm: the persona is the WHOLE prompt,
    /// not the slot between identity and workflow.
    #[test]
    fn a_complete_persona_is_the_whole_prompt() {
        assert_eq!(
            compose_system_prompt_with("You are a helpful software engineer assistant.", true),
            "You are a helpful software engineer assistant."
        );
        let complete = compose_system_prompt_with("You are terse.", true);
        assert!(!complete.contains(HARNESS_IDENTITY));
        assert!(!complete.contains(WORKFLOW_GUIDANCE));
        assert!(!complete.contains(DEFAULT_PERSONA));
        // The default stays the slot: a persona cannot drop the bar unless the
        // caller asked for it explicitly.
        assert_eq!(
            compose_system_prompt_with("You are terse.", false),
            compose_system_prompt("You are terse.")
        );
        assert!(compose_system_prompt("You are terse.").contains(WORKFLOW_GUIDANCE));
        // An empty persona under complete still gets the framework default.
        assert_eq!(
            compose_system_prompt_with("  ", true),
            DEFAULT_SYSTEM_PROMPT
        );
    }

    #[test]
    fn the_default_prompt_pins_every_workflow_rule() {
        for needle in [
            "Read the intent",
            "Act on the obvious reading",
            "Ask only when the decision is the reader's",
            "steers this task",
            "Do the work",
            "Start now",
            "Plan once",
            "Read before you change",
            "Verify before you claim done",
            "Report",
            "Lead with the outcome",
        ] {
            assert!(
                DEFAULT_SYSTEM_PROMPT.contains(needle),
                "the default prompt keeps the rule: {needle}"
            );
        }
    }

    /// The contract is ordered the way a turn runs, and the intent rules that
    /// keep a turn from stalling are all present.
    #[test]
    fn the_contract_reads_intent_before_work_and_reporting() {
        let intent = WORKFLOW_GUIDANCE
            .find("## Read the intent")
            .expect("the intent section");
        let work = WORKFLOW_GUIDANCE
            .find("## Do the work")
            .expect("the work section");
        let report = WORKFLOW_GUIDANCE
            .find("## Report")
            .expect("the report section");
        assert!(intent < work && work < report);
        for rule in [
            "Do the independent work first",
            "one short, self-contained question",
            "Do not ask the reader to confirm what the session already authorized",
            "A message that arrives while you work steers this task",
            "Workspace instructions and the reader's explicit choices outrank these defaults",
            "Stay in scope",
            "do not commit, push or open a pull request",
            "no filler, praise, or invented labels",
        ] {
            assert!(
                WORKFLOW_GUIDANCE.contains(rule),
                "missing intent rule: {rule}"
            );
        }
    }

    /// A static prompt rides on every step, so its size is a budget, not a
    /// taste: the framework default stays far below the BSH build's 12 KB cap.
    #[test]
    fn the_framework_prompt_stays_small() {
        assert!(
            DEFAULT_SYSTEM_PROMPT.len() < 6_000,
            "the framework prompt rides on every step: {} bytes",
            DEFAULT_SYSTEM_PROMPT.len()
        );
    }

    #[test]
    fn guidance_names_only_tools_this_crate_owns() {
        for (name, text) in TOOL_GUIDANCE {
            assert!(!text.trim().is_empty(), "{name} guidance is not empty");
            assert_eq!(guidance_for_tool(name), Some(*text));
        }
        assert_eq!(guidance_for_tool("not_a_tool"), None);
    }

    /// Standing guidance names no tool by name: a host's tool set is its own,
    /// and a rule that says `edit_file` stops being true the moment a host
    /// names its editor something else.
    #[test]
    fn the_workflow_rules_do_not_name_a_tool_a_host_may_not_have() {
        for text in [WORKFLOW_GUIDANCE, DEFAULT_SYSTEM_PROMPT] {
            for name in [
                "edit_file",
                "read_file",
                "write_file",
                "update_plan",
                "bash",
                "`",
            ] {
                assert!(
                    !text.contains(name),
                    "the workflow rules stay name-agnostic, found {name}: {text}"
                );
            }
        }
    }

    /// The discovery paragraph is the one block that names two tools, and it
    /// names exactly the pair the agent advertises with it.
    #[test]
    fn the_discovery_guidance_names_the_pair_it_uses() {
        assert!(DISCOVERY_GUIDANCE.contains("find_tool"));
        assert!(DISCOVERY_GUIDANCE.contains("call_tool"));
        assert!(!DISCOVERY_GUIDANCE.trim().is_empty());
    }

    /// The skill paragraph names the pair a searched install actually carries.
    #[test]
    fn the_skill_guidance_names_the_pair_it_uses() {
        assert!(SKILL_DISCOVERY_GUIDANCE.contains("find_skill"));
        assert!(SKILL_DISCOVERY_GUIDANCE.contains("load_skill"));
        assert!(!SKILL_DISCOVERY_GUIDANCE.trim().is_empty());
    }

    /// The prompt must not name a tool the request does not carry.
    #[test]
    fn no_stale_reference_to_a_removed_tool() {
        let corpus = [
            DEFAULT_SYSTEM_PROMPT,
            WORKFLOW_GUIDANCE,
            TOOL_BASH,
            TOOL_JOBS,
            TOOL_PLAN,
            DISCOVERY_GUIDANCE,
            SKILL_DISCOVERY_GUIDANCE,
        ];
        let forbidden = [
            "`glob`",
            "`grep`",
            "`todo_write`",
            "`exit_plan_mode`",
            "`ask_user_question`",
            "`web_fetch`",
            "`run_code`",
            "the read tool",
            "the write tool",
            "the edit tool",
        ];
        for text in corpus {
            for needle in forbidden {
                assert!(
                    !text.contains(needle),
                    "stale tool reference {needle} in: {text}"
                );
            }
        }
    }
}
