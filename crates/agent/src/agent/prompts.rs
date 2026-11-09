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
/// It is a decision procedure ordered the way a turn runs: resolve what is
/// being asked and which instruction wins, act on that reading, then report the
/// result. The reference spreads these rules across tool sections and runtime
/// context; BSH sends one always-on block because a preset can remove the tool
/// that would otherwise carry a rule, and a static prompt cannot interpolate
/// per-turn facts. `docs/bsh-prompts.md` records the source of each clause and
/// the budget the block lives under.
pub const WORKFLOW_GUIDANCE: &str = "\
You and the reader share one workspace. Turn what they asked for into a finished, verified result.

## Resolve
- Judge the verb, not the punctuation: \"why does X fail?\" asks for an explanation, \"can you fix X?\" asks for a change. A defect report is reproduced, fixed and verified; a terse instruction with a file open means change that code; a goal with no target (\"make it faster\") is measured before it is changed.
- Name the object (what file, screen or behavior), the change (what about it) and the done condition (what proves it) before you act. If the request and the workspace cannot supply them, it is too vague to act on: ask one short question, in the reader's terms, that offers the two or three readings you can see, best first, says what you will do by default, and stop. \"Make it better\" could mean faster, simpler or more correct; a guess that costs a rewrite is not cheap.
- A question ends without an edit; a change request ends with the change made, not instructions for making it.
- When instructions conflict, obey the reader's latest explicit instruction, then a correction they accepted, then workspace instructions and loaded skills, then this contract.
- If the request is concrete but two readings remain, take the one that acts on the workspace when a wrong guess is cheap to undo; ask when it would delete, publish or spend something the reader owns, or when a wrong guess means substantial rework.
- Never ask for a fact the workspace can answer, and never ask twice for what the session already authorized.
- A later message corrects the current task; it replaces the task only when it cancels it or asks for something incompatible.

## Act
- Start now, batch independent calls, and never spend a turn narrating the next step.
- Read before you edit; make the smallest change that satisfies the request; follow the workspace's existing patterns.
- Prove it with the command that checks it, or say what you could not check. After two identical failures, stop and report the error and what must change in the environment.
- Stop when the request is answered or the change is verified. Do not refactor, commit, push or open a pull request unless asked.

## Report
- Outcome first, then evidence, then anything unverified. No preamble or process narration.
- Plain words, concrete nouns, file paths the reader can open; no filler or praise.
- Match the reader's language; brief by default, thorough when the decision needs it.";

/// What the model is told when some tools are withheld from its tool list.
///
/// Appended only when the agent actually withholds a tool: a prompt that named
/// `find_tool` on an agent that does not carry it would be the stale-reference
/// mistake the workflow guidance itself warns about.
///
/// The wording names the two tools by their registered names and nothing else,
/// so it stays true when a host renames its file tools or adds an MCP server.
pub const DISCOVERY_GUIDANCE: &str = "You are given a subset of this machine's tools. When the tool you need is not among them, do not guess: call find_tool with what you need to do, or with an exact tool name to get that tool's full schema. With no query, find_tool lists the names it can offer. Every tool it reports - name, description, and JSON Schema for its arguments - is added to your tool list for the next step. Then call that tool directly by its own name with args matching the reported schema. A withheld tool is not callable before find_tool reports it. Do not conclude a capability is missing before searching for it.";

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

You and the reader share one workspace. Turn what they asked for into a finished, verified result.

## Resolve
- Judge the verb, not the punctuation: \"why does X fail?\" asks for an explanation, \"can you fix X?\" asks for a change. A defect report is reproduced, fixed and verified; a terse instruction with a file open means change that code; a goal with no target (\"make it faster\") is measured before it is changed.
- Name the object (what file, screen or behavior), the change (what about it) and the done condition (what proves it) before you act. If the request and the workspace cannot supply them, it is too vague to act on: ask one short question, in the reader's terms, that offers the two or three readings you can see, best first, says what you will do by default, and stop. \"Make it better\" could mean faster, simpler or more correct; a guess that costs a rewrite is not cheap.
- A question ends without an edit; a change request ends with the change made, not instructions for making it.
- When instructions conflict, obey the reader's latest explicit instruction, then a correction they accepted, then workspace instructions and loaded skills, then this contract.
- If the request is concrete but two readings remain, take the one that acts on the workspace when a wrong guess is cheap to undo; ask when it would delete, publish or spend something the reader owns, or when a wrong guess means substantial rework.
- Never ask for a fact the workspace can answer, and never ask twice for what the session already authorized.
- A later message corrects the current task; it replaces the task only when it cancels it or asks for something incompatible.

## Act
- Start now, batch independent calls, and never spend a turn narrating the next step.
- Read before you edit; make the smallest change that satisfies the request; follow the workspace's existing patterns.
- Prove it with the command that checks it, or say what you could not check. After two identical failures, stop and report the error and what must change in the environment.
- Stop when the request is answered or the change is verified. Do not refactor, commit, push or open a pull request unless asked.

## Report
- Outcome first, then evidence, then anything unverified. No preamble or process narration.
- Plain words, concrete nouns, file paths the reader can open; no filler or praise.
- Match the reader's language; brief by default, thorough when the decision needs it.";

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
            "Judge the verb, not the punctuation",
            "Name the object",
            "too vague to act on",
            "offers the two or three readings",
            "costs a rewrite",
            "A question ends without an edit",
            "a change request ends with the change made",
            "When instructions conflict",
            "when a wrong guess is cheap to undo",
            "substantial rework",
            "delete, publish or spend",
            "never ask twice",
            "A later message corrects the current task",
            "Start now",
            "smallest change",
            "Prove it with the command",
            "After two identical failures",
            "Stop when the request is answered",
            "Do not refactor, commit, push",
            "Outcome first",
            "Plain words",
            "Match the reader's language",
        ] {
            assert!(
                DEFAULT_SYSTEM_PROMPT.contains(needle),
                "the default prompt keeps the rule: {needle}"
            );
        }
    }

    /// The contract is ordered the way a turn runs, and each intent decision
    /// has a clause: what kind of request this is, which instruction wins, when
    /// to ask, when to act on a guess, and when the turn is done.
    #[test]
    fn the_contract_resolves_then_acts_then_reports() {
        let resolve = WORKFLOW_GUIDANCE
            .find("## Resolve")
            .expect("the resolve section");
        let act = WORKFLOW_GUIDANCE.find("## Act").expect("the act section");
        let report = WORKFLOW_GUIDANCE
            .find("## Report")
            .expect("the report section");
        assert!(resolve < act && act < report);
        for rule in [
            "Judge the verb, not the punctuation",
            "Name the object (what file, screen or behavior)",
            "the done condition (what proves it)",
            "too vague to act on",
            "offers the two or three readings you can see, best first",
            "A question ends without an edit",
            "a change request ends with the change made",
            "latest explicit instruction",
            "then workspace instructions and loaded skills, then this contract",
            "when a wrong guess is cheap to undo",
            "substantial rework",
            "never ask twice",
            "A later message corrects the current task",
            "Stop when the request is answered",
            "Do not refactor, commit, push",
            "no filler or praise",
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

    /// The discovery paragraph names the one search tool the agent carries and
    /// says the reported tools join the list for a direct call.
    #[test]
    fn the_discovery_guidance_names_the_search_it_uses() {
        assert!(DISCOVERY_GUIDANCE.contains("find_tool"));
        assert!(!DISCOVERY_GUIDANCE.contains("call_tool"));
        assert!(DISCOVERY_GUIDANCE.contains("added to your tool list"));
        assert!(DISCOVERY_GUIDANCE.contains("directly"));
        assert!(DISCOVERY_GUIDANCE.contains("JSON Schema"));
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
