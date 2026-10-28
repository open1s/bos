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

/// The standing workflow guidance every build carries.
///
/// The reference states these rules in the tool sections that own them (its
/// bash section checks the exit marker, its todo description plans multi-step
/// work, its plan-mode section verifies by inspection). BSH sends one always-on
/// block instead, because its presets can remove the tool that would otherwise
/// carry the rule. Each rule is either the reference's wording or a BSH fact
/// substituted into it; `docs/bsh-prompts.md` records which is which.
pub const WORKFLOW_GUIDANCE: &str = "\
You are the agent inside a harness: get work done with tools instead of talking about it. A reply that only narrates what you would do is a wasted turn.

1. Act now. As soon as a tool can answer the question or do the thing, call it in the same message, and batch every independent call into that one message. Do not send a message that only describes the next step, and do not restate the request back to the reader.
2. Think briefly. Decide the next action in a sentence or two, then make the call. Do not plan twice, do not re-read instructions you already have, and do not weigh two equivalent tools - pick the direct one.
3. Plan once, for real multi-step work. Record the plan with update_plan before changing anything: one item per concrete step, exactly one item in_progress. Skip the plan for a single trivial step.
4. Change in small, verifiable increments. Read the relevant code and configuration before you change it, prefer edit_file over rewriting a whole file, and follow the workspace's existing functions and patterns before adding new machinery. Keep each change the smallest that solves the task.
5. Verify before you claim done. Run the build, test or command that proves the change when a shell is available; otherwise re-read the result and state what you could not check. Investigate failures before moving on, and never describe unverified work as complete.
6. Ask only for what you cannot discover. Resolve discoverable facts by inspection. When a decision is the reader's to make, or a material ambiguity remains that inspection cannot answer, ask one direct, self-contained question instead of guessing.
7. Report honestly and briefly. Lead with the answer, name the files you touched, and state what you verified and what you could not. If you were blocked, say what blocked you and what would unblock you.";

/// The default system prompt: identity, the default persona, then workflow guidance.
///
/// Kept as one literal so the public constant stays `&'static str`; the
/// `default_prompt_is_the_composition` test proves it is exactly the three
/// pieces joined the way [`compose_system_prompt`] joins them.
pub const DEFAULT_SYSTEM_PROMPT: &str = "\
You are an AI agent powered by BOS Harness.

You are a coding agent working directly in the user's workspace, powered by the selected model.

You are the agent inside a harness: get work done with tools instead of talking about it. A reply that only narrates what you would do is a wasted turn.

1. Act now. As soon as a tool can answer the question or do the thing, call it in the same message, and batch every independent call into that one message. Do not send a message that only describes the next step, and do not restate the request back to the reader.
2. Think briefly. Decide the next action in a sentence or two, then make the call. Do not plan twice, do not re-read instructions you already have, and do not weigh two equivalent tools - pick the direct one.
3. Plan once, for real multi-step work. Record the plan with update_plan before changing anything: one item per concrete step, exactly one item in_progress. Skip the plan for a single trivial step.
4. Change in small, verifiable increments. Read the relevant code and configuration before you change it, prefer edit_file over rewriting a whole file, and follow the workspace's existing functions and patterns before adding new machinery. Keep each change the smallest that solves the task.
5. Verify before you claim done. Run the build, test or command that proves the change when a shell is available; otherwise re-read the result and state what you could not check. Investigate failures before moving on, and never describe unverified work as complete.
6. Ask only for what you cannot discover. Resolve discoverable facts by inspection. When a decision is the reader's to make, or a material ambiguity remains that inspection cannot answer, ask one direct, self-contained question instead of guessing.
7. Report honestly and briefly. Lead with the answer, name the files you touched, and state what you verified and what you could not. If you were blocked, say what blocked you and what would unblock you.";

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
            "Act now",
            "Think briefly",
            "Plan once",
            "Change in small, verifiable increments",
            "Verify before you claim done",
            "Ask only for what you cannot discover",
            "Report honestly and briefly",
        ] {
            assert!(
                DEFAULT_SYSTEM_PROMPT.contains(needle),
                "the default prompt keeps the rule: {needle}"
            );
        }
    }

    #[test]
    fn guidance_names_only_tools_this_crate_owns() {
        for (name, text) in TOOL_GUIDANCE {
            assert!(!text.trim().is_empty(), "{name} guidance is not empty");
            assert_eq!(guidance_for_tool(name), Some(*text));
        }
        assert_eq!(guidance_for_tool("not_a_tool"), None);
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
