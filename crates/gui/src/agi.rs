//! AGI-facing surfaces: self-inspection and transcript evaluation.
//!
//! Two read-only probes that let the harness reason about itself:
//!
//! * [`inspect`] reads the agent's own composition — how many tools, skills,
//!   plugins, MCP servers, and memories it has, and what goal it is pursuing;
//! * [`evaluate`] is a deterministic critic over a session transcript, scoring
//!   it from the recorded turns and tool calls with an explicit reason for every
//!   adjustment.
//!
//! Both are pure functions over data the caller already holds, so the numbers
//! are reproducible and testable without a live model.

use agent::goal::Goal;
use serde::Serialize;

use crate::caps::Capabilities;
use crate::session::{Role, SessionRecord};
use crate::settings::Settings;

/// Counts describing what the agent is made of.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct SelfInspection {
    /// Synchronous tools registered on the agent.
    pub(crate) tools: usize,
    /// Asynchronous tools (including MCP-backed) registered on the agent.
    pub(crate) async_tools: usize,
    /// Skills loaded from the skills directory.
    pub(crate) skills: usize,
    /// Registered agent plugins.
    pub(crate) plugins: usize,
    /// Hook events the agent can attach to.
    pub(crate) hook_events: usize,
    /// Connected MCP servers.
    pub(crate) mcp_servers: usize,
    /// Tools contributed by those servers.
    pub(crate) mcp_tools: usize,
    /// Memories the shared store holds.
    pub(crate) memories: usize,
    /// Scoped approval rules in force.
    pub(crate) approval_rules: usize,
    /// The active goal's objective, when one is set.
    pub(crate) active_goal: Option<String>,
    /// The active goal's state name, when one is set.
    pub(crate) goal_state: Option<String>,
    /// Rounds the goal has spent.
    pub(crate) goal_rounds: Option<u32>,
    /// Rounds the goal has left, when it is capped.
    pub(crate) goal_remaining: Option<u32>,
    /// The model this workspace runs.
    pub(crate) model: String,
    /// The active workspace root (empty when none is set).
    pub(crate) workspace: String,
    /// Whether the bash tool is enabled.
    pub(crate) bash_enabled: bool,
    /// Whether the filesystem tools are enabled.
    pub(crate) file_tools_enabled: bool,
    /// The skills directory the agent was built from.
    pub(crate) skills_dir: String,
}

/// Read the agent's composition from its capability snapshot and store counts.
pub(crate) fn inspect(
    capabilities: &Capabilities,
    memories: usize,
    goal: Option<&Goal>,
    settings: &Settings,
    mcp_servers: usize,
    approval_rules: usize,
) -> SelfInspection {
    SelfInspection {
        tools: capabilities.tools.len(),
        async_tools: capabilities.async_tools.len(),
        skills: capabilities.skills.len(),
        plugins: capabilities.plugins.len(),
        hook_events: capabilities.hook_events.len(),
        mcp_servers,
        mcp_tools: capabilities
            .async_tools
            .iter()
            .filter(|tool| tool.category == "mcp")
            .count(),
        memories,
        approval_rules,
        active_goal: goal.map(|goal| goal.objective.clone()),
        goal_state: goal.map(|goal| {
            serde_json::to_value(goal.state)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default()
        }),
        goal_rounds: goal.map(|goal| goal.rounds),
        goal_remaining: goal.and_then(Goal::remaining),
        model: settings.model.clone(),
        workspace: settings.bash_workspace.trim().to_string(),
        bash_enabled: capabilities.bash_enabled,
        file_tools_enabled: capabilities.file_tools_enabled,
        skills_dir: capabilities.skills_dir.clone(),
    }
}

/// The raw signals the critic scored, reported alongside the score.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ScoreSignals {
    /// All turns, folded and live.
    pub(crate) turns: usize,
    /// User turns.
    pub(crate) user_turns: usize,
    /// Assistant turns.
    pub(crate) assistant_turns: usize,
    /// Tool calls made.
    pub(crate) tool_calls: usize,
    /// Tool calls whose recorded output reads as a failure.
    pub(crate) tool_failures: usize,
    /// Turns that recorded a turn-level error.
    pub(crate) errors: usize,
    /// Assistant placeholder turns that never got any text.
    pub(crate) blank_assistant_turns: usize,
    /// Turns that recorded a reasoning trace.
    pub(crate) reasoning_turns: usize,
    /// Whether the last non-blank assistant turn carried visible text.
    pub(crate) final_answer_present: bool,
}

/// A deterministic assessment of one transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TranscriptScore {
    /// Session that was scored.
    pub(crate) session_id: String,
    /// Score from 0 to 100.
    pub(crate) score: i32,
    /// Coarse band: `excellent`, `good`, `fair`, or `poor`.
    pub(crate) grade: String,
    /// One line per adjustment, in a fixed order.
    pub(crate) reasons: Vec<String>,
    /// The counts the score was derived from.
    pub(crate) signals: ScoreSignals,
}

/// Largest deduction for failed tool calls.
const MAX_TOOL_PENALTY: i32 = 30;
/// Largest deduction for turn-level errors.
const MAX_ERROR_PENALTY: i32 = 45;
/// Largest deduction for blank assistant turns.
const MAX_BLANK_PENALTY: i32 = 20;
/// Largest bonus for tool calls that succeeded.
const MAX_TOOL_BONUS: i32 = 10;
/// Largest bonus for turns that show their reasoning.
const MAX_REASONING_BONUS: i32 = 5;

/// Collect the signals a score is built from.
fn signals(record: &SessionRecord) -> ScoreSignals {
    let mut signals = ScoreSignals {
        turns: 0,
        user_turns: 0,
        assistant_turns: 0,
        tool_calls: 0,
        tool_failures: 0,
        errors: 0,
        blank_assistant_turns: 0,
        reasoning_turns: 0,
        final_answer_present: false,
    };
    for message in record.archived.iter().chain(record.messages.iter()) {
        signals.turns += 1;
        match message.role {
            Role::User => signals.user_turns += 1,
            Role::Assistant => signals.assistant_turns += 1,
        }
        if !message.reasoning.trim().is_empty() {
            signals.reasoning_turns += 1;
        }
        if message.role == Role::Assistant && message.text.trim().is_empty() {
            signals.blank_assistant_turns += 1;
        }
        if message.error.is_some() {
            signals.errors += 1;
        }
        for tool in &message.tools {
            signals.tool_calls += 1;
            if tool
                .output
                .as_deref()
                .is_some_and(crate::replay::tool_failed)
            {
                signals.tool_failures += 1;
            }
        }
    }
    signals.final_answer_present = record
        .messages
        .iter()
        .rev()
        .find(|message| message.role == Role::Assistant && !message.is_blank())
        .is_some_and(|message| !message.text.trim().is_empty());
    signals
}

/// The names of the failed tools, in first-seen order.
fn failed_tools(record: &SessionRecord) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for message in record.archived.iter().chain(record.messages.iter()) {
        for tool in &message.tools {
            if tool
                .output
                .as_deref()
                .is_some_and(crate::replay::tool_failed)
                && !names.contains(&tool.name)
            {
                names.push(tool.name.clone());
            }
        }
    }
    names
}

/// Score a session transcript, deterministically and with reasons.
///
/// The scale is a straight 100 with capped deductions and small bonuses, so the
/// number is easy to explain. Folded (`archived`) turns count, because the
/// transcript being judged is the whole conversation, not just what is live.
pub(crate) fn evaluate(record: &SessionRecord) -> TranscriptScore {
    let signals = signals(record);
    let mut score: i32 = 100;
    let mut reasons: Vec<String> = Vec::new();

    if signals.user_turns == 0 {
        score -= 25;
        reasons.push("no user turn to work from".to_string());
    }
    if signals.assistant_turns == 0 {
        score -= 25;
        reasons.push("no assistant turn was produced".to_string());
    }
    if signals.errors > 0 {
        let penalty = (signals.errors as i32 * 15).min(MAX_ERROR_PENALTY);
        score -= penalty;
        reasons.push(format!(
            "{} turn(s) recorded an error ({penalty} off)",
            signals.errors
        ));
    }
    if signals.tool_failures > 0 {
        let penalty = (signals.tool_failures as i32 * 10).min(MAX_TOOL_PENALTY);
        score -= penalty;
        reasons.push(format!(
            "{} tool call(s) failed: {} ({penalty} off)",
            signals.tool_failures,
            failed_tools(record).join(", ")
        ));
    }
    if signals.blank_assistant_turns > 0 {
        let penalty = (signals.blank_assistant_turns as i32 * 5).min(MAX_BLANK_PENALTY);
        score -= penalty;
        reasons.push(format!(
            "{} assistant turn(s) produced no text ({penalty} off)",
            signals.blank_assistant_turns
        ));
    }
    if signals.assistant_turns > 0 && !signals.final_answer_present {
        score -= 10;
        reasons.push("the last assistant turn has no visible answer".to_string());
    }
    let successful_tools = signals.tool_calls - signals.tool_failures;
    if successful_tools > 0 {
        let bonus = (successful_tools as i32 * 2).min(MAX_TOOL_BONUS);
        score += bonus;
        reasons.push(format!(
            "{successful_tools} tool call(s) completed (+{bonus})"
        ));
    }
    if signals.reasoning_turns > 0 {
        let bonus = (signals.reasoning_turns as i32).min(MAX_REASONING_BONUS);
        score += bonus;
        reasons.push(format!(
            "{} turn(s) show their reasoning (+{bonus})",
            signals.reasoning_turns
        ));
    }
    if reasons.is_empty() {
        reasons.push("nothing to fault".to_string());
    }

    let score = score.clamp(0, 100);
    TranscriptScore {
        session_id: record.id.clone(),
        score,
        grade: match score {
            85..=100 => "excellent",
            70..=84 => "good",
            55..=69 => "fair",
            _ => "poor",
        }
        .to_string(),
        reasons,
        signals,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::CapInfo;
    use crate::session::{ChatMessage, ToolEvent};

    fn cap(name: &str) -> CapInfo {
        CapInfo {
            name: name.to_string(),
            description: format!("{name} description"),
            category: "builtin".to_string(),
        }
    }

    fn capabilities() -> Capabilities {
        Capabilities {
            tools: vec![cap("bash"), cap("read_file")],
            async_tools: vec![
                CapInfo {
                    name: "mcp_alpha_search".to_string(),
                    description: String::new(),
                    category: "mcp".to_string(),
                },
                CapInfo {
                    name: "internal_async".to_string(),
                    description: String::new(),
                    category: "async".to_string(),
                },
            ],
            skills: vec![cap("code-review")],
            plugins: vec!["audit".to_string()],
            hook_events: vec!["pre_tool".to_string(), "post_tool".to_string()],
            skills_dir: "/skills".to_string(),
            bash_enabled: true,
            file_tools_enabled: false,
        }
    }

    /// Self-inspection counts every surface from the snapshot it is given.
    #[test]
    fn self_inspection_counts_every_surface() {
        let settings = Settings {
            model: "test-model".to_string(),
            bash_workspace: "/work".to_string(),
            ..Settings::default()
        };
        let goal = Goal::new("ship milestone B", Some(10)).expect("goal");
        let report = inspect(&capabilities(), 7, Some(&goal), &settings, 1, 3);

        assert_eq!(report.tools, 2);
        assert_eq!(report.async_tools, 2);
        assert_eq!(report.skills, 1);
        assert_eq!(report.plugins, 1);
        assert_eq!(report.hook_events, 2);
        assert_eq!(report.mcp_servers, 1);
        assert_eq!(report.mcp_tools, 1, "only the mcp-categorised async tool");
        assert_eq!(report.memories, 7);
        assert_eq!(report.approval_rules, 3);
        assert_eq!(report.active_goal.as_deref(), Some("ship milestone B"));
        assert_eq!(report.goal_state.as_deref(), Some("active"));
        assert_eq!(report.goal_rounds, Some(0));
        assert_eq!(report.goal_remaining, Some(10));
        assert_eq!(report.model, "test-model");
        assert_eq!(report.workspace, "/work");
        assert!(report.bash_enabled);
        assert!(!report.file_tools_enabled);
        assert_eq!(report.skills_dir, "/skills");
    }

    /// With no goal set, the goal fields are absent rather than zero.
    #[test]
    fn self_inspection_reports_no_goal_as_none() {
        let report = inspect(&capabilities(), 0, None, &Settings::default(), 0, 0);
        assert!(report.active_goal.is_none());
        assert!(report.goal_state.is_none());
        assert!(report.goal_rounds.is_none());
        assert!(report.goal_remaining.is_none());
        let json = serde_json::to_value(&report).unwrap();
        assert!(json["active_goal"].is_null());
        assert_eq!(json["memories"], 0);
    }

    /// A clean transcript scores high and says why.
    #[test]
    fn a_clean_transcript_scores_well() {
        let mut record = SessionRecord::new();
        record
            .messages
            .push(ChatMessage::user("please run the checks"));
        let mut answer = ChatMessage::assistant();
        answer.text = "all checks passed".to_string();
        answer.reasoning = "I ran each one".to_string();
        answer.tools.push(ToolEvent {
            name: "bash".into(),
            args: "{\"command\":\"cargo test\"}".into(),
            output: Some("ok".into()),
            ms: Some(4),
        });
        record.messages.push(answer);

        let scored = evaluate(&record);
        assert_eq!(scored.signals.tool_calls, 1);
        assert_eq!(scored.signals.tool_failures, 0);
        assert!(scored.signals.final_answer_present);
        assert!(
            scored.score >= 90,
            "clean run should score well: {}",
            scored.score
        );
        assert_eq!(scored.grade, "excellent");
        assert!(scored.reasons.iter().any(|r| r.contains("completed")));
    }

    /// A transcript with a failing tool call scores strictly lower.
    #[test]
    fn a_failing_tool_call_scores_lower() {
        let mut clean = SessionRecord::new();
        clean.messages.push(ChatMessage::user("do the thing"));
        let mut ok = ChatMessage::assistant();
        ok.text = "done".to_string();
        ok.tools.push(ToolEvent {
            name: "bash".into(),
            args: "{}".into(),
            output: Some("fine".into()),
            ms: Some(1),
        });
        clean.messages.push(ok);

        let mut failing = clean.clone();
        failing.messages[1].tools[0].output = Some("Failed: exit status 1".into());
        failing.messages[1].error = Some("the command failed".into());

        let good = evaluate(&clean);
        let bad = evaluate(&failing);
        assert!(
            bad.score < good.score,
            "failing run must score lower ({} vs {})",
            bad.score,
            good.score
        );
        assert_eq!(bad.signals.tool_failures, 1);
        assert_eq!(bad.signals.errors, 1);
        assert!(
            bad.reasons.iter().any(|r| r.contains("failed")),
            "the reason must name the failure: {:?}",
            bad.reasons
        );
    }

    /// Blank assistant placeholders are penalised, but do not zero the score.
    #[test]
    fn blank_assistant_turns_are_penalised() {
        let mut record = SessionRecord::new();
        record.messages.push(ChatMessage::user("hello"));
        record.messages.push(ChatMessage::assistant());
        record.messages.push(ChatMessage::user("are you there?"));
        record.messages.push(ChatMessage::assistant());
        let scored = evaluate(&record);
        assert_eq!(scored.signals.blank_assistant_turns, 2);
        assert!(!scored.signals.final_answer_present);
        assert!(scored.score < 100);
        assert!(
            scored.reasons.iter().any(|r| r.contains("no text")),
            "{:?}",
            scored.reasons
        );
    }

    /// An empty transcript scores poorly but never panics or goes negative.
    #[test]
    fn an_empty_transcript_scores_poorly_and_stays_bounded() {
        let scored = evaluate(&SessionRecord::new());
        assert_eq!(scored.signals.turns, 0);
        assert_eq!(scored.score, 50, "empty history loses both halves");
        assert_eq!(scored.grade, "poor");
        assert!(scored.reasons.iter().any(|r| r.contains("no user turn")));
        assert!((0..=100).contains(&scored.score), "score stays in range");
    }

    /// The evaluation is deterministic and flat-serializable.
    #[test]
    fn evaluation_is_deterministic_and_serializes_flat() {
        let mut record = SessionRecord::new();
        record.messages.push(ChatMessage::user("q"));
        let mut answer = ChatMessage::assistant();
        answer.text = "a".to_string();
        record.messages.push(answer);
        let first = serde_json::to_string(&evaluate(&record)).unwrap();
        let second = serde_json::to_string(&evaluate(&record)).unwrap();
        assert_eq!(first, second);
        let json: serde_json::Value = serde_json::from_str(&first).unwrap();
        for key in ["session_id", "score", "grade", "reasons", "signals"] {
            assert!(json.get(key).is_some(), "missing {key}");
        }
    }
}
