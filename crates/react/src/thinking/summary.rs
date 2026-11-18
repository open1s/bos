//! The typed thinking summary and its provider-facing rendering.

use serde::{Deserialize, Serialize};

use super::{clip_chars, one_line, THINKING_SCHEMA_VERSION, THINKING_SOURCE};

/// Where the summary is in the turn it describes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingPhase {
    /// The model is still gathering evidence.
    #[default]
    Exploring,
    /// A direction has been chosen but no work has run yet.
    Planning,
    /// Tool calls are running.
    Implementing,
    /// Results are being checked against the plan.
    Verifying,
    /// The last attempts failed and the next step is diagnosis.
    Blocked,
    /// The turn closed; nothing from this turn is injected again.
    Complete,
}

impl ThinkingPhase {
    /// The wire name of this phase.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Exploring => "exploring",
            Self::Planning => "planning",
            Self::Implementing => "implementing",
            Self::Verifying => "verifying",
            Self::Blocked => "blocked",
            Self::Complete => "complete",
        }
    }

    /// Whether the turn is closed.
    pub fn is_terminal(&self) -> bool {
        *self == Self::Complete
    }
}

/// How strongly one summary item is supported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingConfidence {
    /// The model stated it, but no observed result corroborates it yet.
    Inferred,
    /// An observed tool result corroborated it.
    Verified,
    /// A later decision or a failed result retired it.
    Superseded,
}

impl ThinkingConfidence {
    /// The label rendered in the injected note.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Inferred => "inferred",
            Self::Verified => "verified",
            Self::Superseded => "superseded",
        }
    }
}

/// What produced an item.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingOrigin {
    /// The model's own reasoning stream.
    #[default]
    Reasoning,
    /// An observed tool result.
    ToolResult,
    /// The reader said it in the conversation.
    Reader,
}

impl ThinkingOrigin {
    /// The wire name of this origin.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Reasoning => "reasoning",
            Self::ToolResult => "tool-result",
            Self::Reader => "reader",
        }
    }
}

/// One judgment the model made, with the reason it gave.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingDecision {
    /// The concise judgment.
    pub statement: String,
    /// Why the model chose it, when the reasoning gave a reason.
    pub rationale: Option<String>,
    /// How strongly the judgment is supported.
    pub confidence: ThinkingConfidence,
    /// What produced the judgment.
    pub origin: ThinkingOrigin,
    /// Observed results that corroborated or contradicted it.
    pub evidence: Vec<String>,
    /// The ReAct step that recorded it.
    pub step: usize,
    /// Whether it is still live. Retired decisions stay for audit but are not
    /// injected.
    pub active: bool,
    /// The statement that replaced this one, when a newer judgment retired it.
    pub superseded_by: Option<String>,
}

impl ThinkingDecision {
    /// Whether this decision is live and injectable.
    pub fn is_live(&self) -> bool {
        self.active && self.confidence != ThinkingConfidence::Superseded
    }
}

/// One question the reasoning left unresolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingOpenQuestion {
    /// The unresolved question.
    pub question: String,
    /// The ReAct step that recorded it.
    pub step: usize,
    /// Whether a later observation answered it.
    pub resolved: bool,
}

/// One tool call awaiting its result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingTool {
    /// The tool that was called.
    pub name: String,
    /// The ReAct step that issued the call.
    pub step: usize,
    /// The next action the reasoning stated before the call, when it did.
    pub action: Option<String>,
}

/// The compact summary injected into later provider calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingSummary {
    /// The engine turn this summary belongs to.
    pub turn: u64,
    /// Monotonic revision within the kept state, bumped on every change.
    pub revision: u64,
    /// Where the turn stands.
    pub phase: ThinkingPhase,
    /// The judgments recorded so far, oldest first.
    pub decisions: Vec<ThinkingDecision>,
    /// Questions the reasoning has not answered.
    pub open_questions: Vec<ThinkingOpenQuestion>,
    /// The one concrete action the model said it would take next.
    pub next_action: Option<String>,
    /// The last ReAct step that changed the summary.
    pub last_step: usize,
}

impl Default for ThinkingSummary {
    fn default() -> Self {
        Self {
            turn: 0,
            revision: 0,
            phase: ThinkingPhase::Exploring,
            decisions: Vec::new(),
            open_questions: Vec::new(),
            next_action: None,
            last_step: 0,
        }
    }
}

impl ThinkingSummary {
    /// Create an empty summary for `turn`.
    pub fn new(turn: u64) -> Self {
        Self {
            turn,
            ..Self::default()
        }
    }

    /// The live decisions, oldest first.
    pub fn active_decisions(&self) -> impl Iterator<Item = &ThinkingDecision> {
        self.decisions.iter().filter(|decision| decision.is_live())
    }

    /// The unresolved questions, oldest first.
    pub fn unresolved_questions(&self) -> impl Iterator<Item = &ThinkingOpenQuestion> {
        self.open_questions
            .iter()
            .filter(|question| !question.resolved)
    }

    /// Whether anything in this summary is worth injecting.
    pub fn has_injectable(&self) -> bool {
        self.active_decisions().next().is_some()
            || self.unresolved_questions().next().is_some()
            || self
                .next_action
                .as_deref()
                .is_some_and(|action| !action.trim().is_empty())
    }

    /// Render the summary as the one host-guided note for a provider call.
    ///
    /// The first line is the explicit source marker. Items carry their own
    /// confidence label, so an unverified inference is never rendered as a
    /// fact. Returns `None` when there is nothing to say.
    pub fn render(&self) -> Option<String> {
        let decisions: Vec<&ThinkingDecision> = self.active_decisions().collect();
        let questions: Vec<&ThinkingOpenQuestion> = self.unresolved_questions().collect();
        let action = self
            .next_action
            .as_deref()
            .map(one_line)
            .filter(|action| !action.is_empty());
        if decisions.is_empty() && questions.is_empty() && action.is_none() {
            return None;
        }

        let mut out = format!(
            "[source: {THINKING_SOURCE} revision={} phase={}]\n",
            self.revision,
            self.phase.as_str()
        );
        out.push_str(
            "This note is distilled from the model's own reasoning and observed tool results. It is not a reader message and not a new instruction. Treat items marked inferred as unverified until a result confirms them.\n",
        );
        if !decisions.is_empty() {
            out.push_str("decisions:\n");
            for decision in decisions {
                out.push_str("- [");
                out.push_str(decision.confidence.as_str());
                out.push_str("] ");
                out.push_str(&clip_chars(
                    &one_line(&decision.statement),
                    super::ITEM_CHARS,
                ));
                if let Some(rationale) = &decision.rationale {
                    let rationale = one_line(rationale);
                    if !rationale.is_empty() {
                        out.push_str(" because ");
                        out.push_str(&clip_chars(&rationale, super::ITEM_CHARS));
                    }
                }
                if let Some(evidence) = decision.evidence.last() {
                    out.push_str(" (evidence: ");
                    out.push_str(&clip_chars(&one_line(evidence), 160));
                    out.push(')');
                }
                out.push('\n');
            }
        }
        if !questions.is_empty() {
            out.push_str("open questions:\n");
            for question in questions {
                out.push_str("- ");
                out.push_str(&clip_chars(
                    &one_line(&question.question),
                    super::QUESTION_CHARS,
                ));
                out.push('\n');
            }
        }
        if let Some(action) = action {
            out.push_str("next action (tentative): ");
            out.push_str(&clip_chars(&action, super::ITEM_CHARS));
            out.push('\n');
        }
        Some(out)
    }
}

/// The serializable state a host can persist with its session.
///
/// It is deliberately a separate wrapper from [`ThinkingSummary`]: the summary
/// is the model-facing content, while the snapshot also carries schema and
/// in-flight observations needed to resume correctly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingSnapshot {
    /// The schema this snapshot was written with.
    pub schema: u32,
    /// The model-facing summary.
    pub summary: ThinkingSummary,
    /// Tool calls that were issued but whose results had not arrived.
    pub pending: Vec<PendingTool>,
    /// Consecutive failed tool results, used to detect a blocked turn.
    pub consecutive_failures: u32,
}

impl Default for ThinkingSnapshot {
    fn default() -> Self {
        Self {
            schema: THINKING_SCHEMA_VERSION,
            summary: ThinkingSummary::default(),
            pending: Vec::new(),
            consecutive_failures: 0,
        }
    }
}

impl ThinkingSnapshot {
    /// Create an empty snapshot for `turn`.
    pub fn new(turn: u64) -> Self {
        Self {
            schema: THINKING_SCHEMA_VERSION,
            summary: ThinkingSummary::new(turn),
            pending: Vec::new(),
            consecutive_failures: 0,
        }
    }

    /// Whether this build can read the snapshot.
    pub fn is_supported(&self) -> bool {
        self.schema > 0 && self.schema <= THINKING_SCHEMA_VERSION
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ThinkingConfidence, ThinkingDecision, ThinkingOpenQuestion, ThinkingOrigin, ThinkingPhase,
        ThinkingSnapshot, ThinkingSummary,
    };

    fn decision(statement: &str, confidence: ThinkingConfidence) -> ThinkingDecision {
        ThinkingDecision {
            statement: statement.to_string(),
            rationale: Some("the failure starts there".to_string()),
            confidence,
            origin: ThinkingOrigin::Reasoning,
            evidence: Vec::new(),
            step: 1,
            active: true,
            superseded_by: None,
        }
    }

    #[test]
    fn the_rendered_note_names_its_source_and_marks_inference() {
        let mut summary = ThinkingSummary::new(1);
        summary.revision = 3;
        summary.phase = ThinkingPhase::Implementing;
        summary.decisions.push(decision(
            "the parser is the cause",
            ThinkingConfidence::Inferred,
        ));
        summary.open_questions.push(ThinkingOpenQuestion {
            question: "whether the cache is stale".to_string(),
            step: 1,
            resolved: false,
        });
        summary.next_action = Some("run the parser test".to_string());
        let note = summary.render().expect("a note");
        assert!(note.starts_with("[source: thinking-summary revision=3"));
        assert!(note.contains("[inferred] the parser is the cause because"));
        assert!(note.contains("open questions:"));
        assert!(note.contains("next action (tentative): run the parser test"));
    }

    #[test]
    fn retired_decisions_are_not_rendered() {
        let mut summary = ThinkingSummary::new(1);
        let mut old = decision("use approach A", ThinkingConfidence::Superseded);
        old.active = false;
        old.superseded_by = Some("use approach B".to_string());
        summary.decisions.push(old);
        assert!(summary.render().is_none());
    }

    #[test]
    fn a_snapshot_round_trips_through_json() {
        let mut snapshot = ThinkingSnapshot::new(2);
        snapshot
            .summary
            .decisions
            .push(decision("fix the retry loop", ThinkingConfidence::Verified));
        let json = serde_json::to_string(&snapshot).expect("serializes");
        let restored: ThinkingSnapshot = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(restored, snapshot);
    }

    #[test]
    fn a_newer_schema_is_refused() {
        let mut snapshot = ThinkingSnapshot::new(1);
        snapshot.schema = super::THINKING_SCHEMA_VERSION + 1;
        assert!(!snapshot.is_supported());
    }
}
