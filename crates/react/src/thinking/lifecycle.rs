//! The thinking lifecycle: ingest reasoning and tool outcomes, keep the
//! summary current, and emit one host-guidance note per request.

use super::extract::{
    bounded_tail, extract_decisions, extract_next_action, extract_questions, first_line,
    is_negated, looks_failed, looks_verifying,
};
use super::summary::{
    PendingTool, ThinkingConfidence, ThinkingDecision, ThinkingOpenQuestion, ThinkingPhase,
    ThinkingSnapshot, ThinkingSummary,
};
use super::{normalize_text, similarity, ThinkingOrigin, THINKING_SOURCE};
use crate::llm::{HostGuidance, LlmMessage, ReactSession};

/// The live lifecycle owner for one agent.
///
/// The engine keeps one of these across turns. It loads its starting state from
/// the session snapshot, updates on reasoning and tool outcomes, and persists
/// the snapshot back after every change so an interrupted run can resume from
/// the still-valid part of the summary.
#[derive(Debug, Clone, Default)]
pub struct ThinkingState {
    snapshot: ThinkingSnapshot,
    /// The (revision, step) already emitted, so one step never receives the
    /// same note twice.
    injected: Option<(u64, usize)>,
}

impl ThinkingState {
    /// Create an empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Load a state from a persisted snapshot, ignoring an unsupported schema.
    pub fn from_snapshot(snapshot: Option<ThinkingSnapshot>) -> Self {
        match snapshot {
            Some(snapshot) if snapshot.is_supported() => Self {
                snapshot,
                injected: None,
            },
            _ => Self::new(),
        }
    }

    /// The current snapshot, for persistence and diagnostics.
    pub fn snapshot(&self) -> &ThinkingSnapshot {
        &self.snapshot
    }

    /// The current model-facing summary.
    pub fn summary(&self) -> &ThinkingSummary {
        &self.snapshot.summary
    }

    /// Start one engine run.
    ///
    /// A completed previous turn keeps only the corroborated decisions and
    /// unresolved questions that still overlap `task`; inferred judgments from
    /// that turn were never verified and are not injected into a new task. An
    /// interrupted turn (anything not `Complete`) resumes exactly where it
    /// stopped, because it is the same task.
    pub fn begin_run(&mut self, turn: u64, task: &str) {
        if self.snapshot.summary.turn == 0 {
            self.snapshot = ThinkingSnapshot::new(turn);
        } else if self.snapshot.summary.phase.is_terminal() {
            let prior = &self.snapshot.summary;
            let decisions: Vec<ThinkingDecision> = prior
                .decisions
                .iter()
                .filter(|decision| {
                    decision.is_live()
                        && decision.confidence == ThinkingConfidence::Verified
                        && task_is_related(task, &decision.statement)
                })
                .cloned()
                .collect();
            let questions: Vec<ThinkingOpenQuestion> = prior
                .open_questions
                .iter()
                .filter(|question| !question.resolved && task_is_related(task, &question.question))
                .cloned()
                .collect();
            let revision = prior.revision.saturating_add(1);
            self.snapshot.summary = ThinkingSummary {
                turn,
                revision,
                phase: ThinkingPhase::Exploring,
                decisions,
                open_questions: questions,
                next_action: None,
                last_step: 0,
            };
        } else {
            self.snapshot.summary.turn = turn;
        }
        self.snapshot.pending.clear();
        self.snapshot.consecutive_failures = 0;
        self.injected = None;
    }

    /// Ingest one reasoning step.
    ///
    /// Returns whether the summary changed. Every extracted decision starts
    /// `Inferred`; only [`Self::observe_tool_result`] can corroborate it.
    pub fn observe_reasoning(&mut self, text: &str, step: usize) -> bool {
        if text.trim().is_empty() {
            return false;
        }
        let tail = bounded_tail(text);
        let decisions = extract_decisions(tail, step);
        let questions = extract_questions(tail, step);
        let action = extract_next_action(tail);
        if decisions.is_empty() && questions.is_empty() && action.is_none() {
            return false;
        }

        for decision in decisions {
            self.merge_decision(decision);
        }
        for question in questions {
            let duplicate = self.snapshot.summary.open_questions.iter().any(|existing| {
                !existing.resolved
                    && (normalize_text(&existing.question) == normalize_text(&question.question)
                        || similarity(&existing.question, &question.question) >= 0.8)
            });
            if !duplicate {
                self.snapshot.summary.open_questions.push(question);
            }
        }
        if let Some(action) = action {
            self.snapshot.summary.next_action = Some(action);
        }
        if self.snapshot.summary.phase == ThinkingPhase::Exploring {
            self.snapshot.summary.phase = ThinkingPhase::Planning;
        }
        self.snapshot.summary.last_step = step;
        self.prune();
        self.bump();
        true
    }

    /// Record a tool call before it runs, keeping the action it was meant to
    /// carry out so a result can corroborate or retire the matching judgment.
    pub fn observe_tool_call(&mut self, name: &str, step: usize) {
        self.snapshot.pending.push(PendingTool {
            name: name.to_string(),
            step,
            action: self.snapshot.summary.next_action.clone(),
        });
        if self.snapshot.summary.phase != ThinkingPhase::Blocked {
            self.snapshot.summary.phase = ThinkingPhase::Implementing;
        }
        self.snapshot.summary.next_action = None;
        self.snapshot.summary.last_step = step;
        self.bump();
    }

    /// Record one tool result and correct the judgments it bears on.
    ///
    /// A success corroborates a related inferred decision. A failure retires
    /// the judgment that led to the call, adds the open question the failure
    /// raises, and sets the next action to diagnosis instead of a retry.
    pub fn observe_tool_result(&mut self, name: &str, output: &str, success: bool, step: usize) {
        let pending = self.take_pending(name);
        let action = pending.and_then(|pending| pending.action);
        let failed = !success || looks_failed(output);

        if failed {
            self.snapshot.consecutive_failures =
                self.snapshot.consecutive_failures.saturating_add(1);
            let evidence = format!("{name} failed at step {step}: {}", first_line(output));
            for decision in &mut self.snapshot.summary.decisions {
                if !decision.is_live() {
                    continue;
                }
                let related = action
                    .as_deref()
                    .is_some_and(|action| similarity(&decision.statement, action) >= 0.3);
                if related {
                    decision.active = false;
                    decision.confidence = ThinkingConfidence::Superseded;
                    decision.evidence.push(evidence.clone());
                    decision.superseded_by = Some(format!("{name} failed at step {step}"));
                }
            }
            let question = format!("Why did {name} fail at step {step}? {}", first_line(output));
            let duplicate = self.snapshot.summary.open_questions.iter().any(|existing| {
                !existing.resolved && similarity(&existing.question, &question) >= 0.8
            });
            if !duplicate {
                self.snapshot
                    .summary
                    .open_questions
                    .push(ThinkingOpenQuestion {
                        question,
                        step,
                        resolved: false,
                    });
            }
            self.snapshot.summary.next_action =
                Some(format!("Diagnose the {name} failure before retrying"));
            self.snapshot.summary.phase = if self.snapshot.consecutive_failures >= 2 {
                ThinkingPhase::Blocked
            } else {
                ThinkingPhase::Implementing
            };
        } else {
            self.snapshot.consecutive_failures = 0;
            let evidence = format!("{name} succeeded at step {step}");
            for decision in &mut self.snapshot.summary.decisions {
                if !decision.is_live() || decision.confidence != ThinkingConfidence::Inferred {
                    continue;
                }
                let related = action
                    .as_deref()
                    .is_some_and(|action| similarity(&decision.statement, action) >= 0.5);
                if related {
                    decision.confidence = ThinkingConfidence::Verified;
                    decision.origin = ThinkingOrigin::ToolResult;
                    decision.evidence.push(evidence.clone());
                }
            }
            self.snapshot.summary.next_action = None;
            self.snapshot.summary.phase = if looks_verifying(output) {
                ThinkingPhase::Verifying
            } else {
                ThinkingPhase::Implementing
            };
        }
        self.snapshot.summary.last_step = step;
        self.prune();
        self.bump();
    }

    /// Close the turn.
    ///
    /// The summary stays available for audit, but a terminal phase is never
    /// injected again.
    pub fn finish(&mut self) {
        if !self.snapshot.summary.phase.is_terminal() {
            self.snapshot.summary.phase = ThinkingPhase::Complete;
            self.snapshot.summary.next_action = None;
            self.snapshot.pending.clear();
            self.snapshot.consecutive_failures = 0;
            self.bump();
        }
    }

    /// Whether the next request should carry a note, and the note itself.
    ///
    /// The step/revision pair suppresses a duplicate within one step. Items
    /// already present in the conversation are dropped, so the note does not
    /// repeat what the history says; if nothing survives, nothing is injected.
    pub fn prepare_injection(
        &mut self,
        step: usize,
        history: &[LlmMessage],
    ) -> Option<HostGuidance> {
        if self.snapshot.summary.phase.is_terminal() || !self.snapshot.summary.has_injectable() {
            return None;
        }
        let revision = self.snapshot.summary.revision;
        if self.injected == Some((revision, step)) {
            return None;
        }
        let mut summary = self.snapshot.summary.clone();
        summary.decisions.retain(|decision| {
            decision.is_live() && !history_contains(history, &decision.statement)
        });
        summary.open_questions.retain(|question| {
            !question.resolved && !history_contains(history, &question.question)
        });
        if summary
            .next_action
            .as_deref()
            .is_some_and(|action| history_contains(history, action))
        {
            summary.next_action = None;
        }
        let content = summary.render()?;
        self.injected = Some((revision, step));
        Some(HostGuidance::new(THINKING_SOURCE, content))
    }

    /// The snapshot to persist, if it has changed since the last save.
    pub fn snapshot_cloned(&self) -> ThinkingSnapshot {
        self.snapshot.clone()
    }

    /// Persist the snapshot into a session that keeps one.
    pub fn persist<S: ReactSession>(&self, session: &mut S) {
        session.set_thinking_snapshot(Some(self.snapshot.clone()));
    }

    /// Merge one new decision into the summary.
    ///
    /// An identical statement refreshes the step. A statement that covers the
    /// same subject (high token overlap, or the same subject with opposite
    /// negation) retires the older one, which is how a changed judgment updates
    /// the summary instead of accumulating both.
    fn merge_decision(&mut self, decision: ThinkingDecision) {
        let normalized = normalize_text(&decision.statement);
        for existing in &mut self.snapshot.summary.decisions {
            if existing.is_live() && normalize_text(&existing.statement) == normalized {
                existing.step = decision.step;
                if existing.rationale.is_none() {
                    existing.rationale = decision.rationale;
                }
                return;
            }
        }
        for existing in &mut self.snapshot.summary.decisions {
            if !existing.is_live() {
                continue;
            }
            let overlap = similarity(&existing.statement, &decision.statement);
            let opposite = is_negated(&existing.statement) != is_negated(&decision.statement);
            if overlap >= 0.6 || (overlap >= 0.5 && opposite) {
                existing.active = false;
                existing.confidence = ThinkingConfidence::Superseded;
                existing.superseded_by = Some(decision.statement.clone());
            }
        }
        self.snapshot.summary.decisions.push(decision);
    }

    /// Keep the summary bounded: retired items go first.
    fn prune(&mut self) {
        if self.snapshot.summary.decisions.len() > 12 {
            while self.snapshot.summary.decisions.len() > 12 {
                let retired = self
                    .snapshot
                    .summary
                    .decisions
                    .iter()
                    .position(|decision| !decision.is_live());
                match retired {
                    Some(index) => {
                        self.snapshot.summary.decisions.remove(index);
                    }
                    None => {
                        self.snapshot.summary.decisions.remove(0);
                    }
                }
            }
        }
        if self.snapshot.summary.open_questions.len() > 8 {
            while self.snapshot.summary.open_questions.len() > 8 {
                let resolved = self
                    .snapshot
                    .summary
                    .open_questions
                    .iter()
                    .position(|question| question.resolved);
                match resolved {
                    Some(index) => {
                        self.snapshot.summary.open_questions.remove(index);
                    }
                    None => {
                        self.snapshot.summary.open_questions.remove(0);
                    }
                }
            }
        }
    }

    fn take_pending(&mut self, name: &str) -> Option<PendingTool> {
        let index = self
            .snapshot
            .pending
            .iter()
            .position(|pending| pending.name == name)?;
        Some(self.snapshot.pending.remove(index))
    }

    fn bump(&mut self) {
        self.snapshot.summary.revision = self.snapshot.summary.revision.saturating_add(1);
    }
}

/// Whether a carried item still bears on the next task.
///
/// A new turn starts from a new request; a verified decision or open question
/// with no word in common with that request is about work the reader has moved
/// on from. An empty task is the caller saying it cannot filter, so everything
/// verified is kept.
fn task_is_related(task: &str, item: &str) -> bool {
    task.trim().is_empty() || similarity(item, task) > 0.0
}

/// Whether the history already carries `text`.
///
/// Only the last 40 rows and a bounded prefix are compared: the check is a
/// duplicate guard, not a search engine, and it must stay cheap on every step.
fn history_contains(history: &[LlmMessage], text: &str) -> bool {
    let needle = normalize_text(text);
    if needle.chars().count() < 12 {
        return false;
    }
    let prefix: String = needle.chars().take(48).collect();
    history
        .iter()
        .rev()
        .take(40)
        .filter_map(|message| match message {
            LlmMessage::System { content } | LlmMessage::Assistant { content } => {
                Some(content.as_str())
            }
            LlmMessage::ToolResult { content, .. } => Some(content.as_str()),
            LlmMessage::User { content } => content.as_text(),
            LlmMessage::AssistantToolCall { .. } => None,
        })
        .any(|body| {
            let body = normalize_text(body);
            body.contains(&needle) || body.contains(&prefix)
        })
}

#[cfg(test)]
mod tests {
    use super::ThinkingState;
    use crate::llm::LlmMessage;
    use crate::thinking::summary::ThinkingConfidence;

    #[test]
    fn a_successful_tool_verifies_the_related_decision() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "");
        assert!(state.observe_reasoning(
            "I will run the parser test because it reproduces the failure.",
            1
        ));
        state.observe_tool_call("bash", 2);
        state.observe_tool_result("bash", "test result: ok. 4 passed; 0 failed", true, 2);
        let decision = state
            .summary()
            .decisions
            .iter()
            .find(|decision| decision.statement.contains("parser test"))
            .expect("the decision survives");
        assert_eq!(decision.confidence, ThinkingConfidence::Verified);
        assert!(decision
            .evidence
            .iter()
            .any(|line| line.contains("succeeded")));
        assert_eq!(state.summary().phase.as_str(), "verifying");
    }

    #[test]
    fn an_unrelated_judgment_is_not_promoted_by_a_success() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "");
        state.observe_reasoning("The cause is the cache because reads are stale.", 1);
        state.observe_tool_call("bash", 2);
        state.observe_tool_result("bash", "test result: ok. 4 passed; 0 failed", true, 2);
        let decision = state
            .summary()
            .decisions
            .iter()
            .find(|decision| decision.statement.contains("cache"))
            .expect("the judgment is recorded");
        assert_eq!(
            decision.confidence,
            ThinkingConfidence::Inferred,
            "a success elsewhere does not turn an unverified claim into a fact"
        );
    }

    #[test]
    fn a_failed_tool_retires_the_judgment_and_asks_why() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "");
        state.observe_reasoning(
            "I will run the parser test because it reproduces the failure.",
            1,
        );
        state.observe_tool_call("bash", 2);
        state.observe_tool_result("bash", "Error: no test named parser found", false, 2);
        assert!(
            state
                .summary()
                .decisions
                .iter()
                .find(|decision| decision.statement.contains("parser test"))
                .is_some_and(|decision| !decision.is_live()),
            "the failed action's judgment is retired"
        );
        assert!(state
            .summary()
            .open_questions
            .iter()
            .any(|question| question.question.contains("no test named parser")));
        assert_eq!(
            state.summary().next_action.as_deref(),
            Some("Diagnose the bash failure before retrying")
        );
    }

    #[test]
    fn a_new_judgment_supersedes_the_one_it_replaces() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "");
        state.observe_reasoning("The fix is to reuse the existing lock.", 1);
        state.observe_reasoning("The fix is to replace the existing lock.", 2);
        let live: Vec<&str> = state
            .summary()
            .active_decisions()
            .map(|decision| decision.statement.as_str())
            .collect();
        assert_eq!(live.len(), 1, "only the newer judgment is live: {live:?}");
        assert!(live[0].contains("replace"));
    }

    #[test]
    fn a_note_that_repeats_the_history_is_not_injected() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "");
        state.observe_reasoning("The fix is to reuse the existing lock.", 1);
        let history = vec![LlmMessage::assistant(
            "The fix is to reuse the existing lock.",
        )];
        assert!(state.prepare_injection(2, &history).is_none());
    }

    #[test]
    fn a_completed_turn_never_injects() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "");
        state.observe_reasoning("The fix is to reuse the existing lock.", 1);
        state.finish();
        assert!(state.prepare_injection(2, &[]).is_none());
    }

    #[test]
    fn a_new_turn_keeps_verified_decisions_but_drops_inference() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "");
        state.observe_reasoning("I will run the parser test because it fails.", 1);
        state.observe_tool_call("bash", 2);
        state.observe_tool_result("bash", "test result: ok. 4 passed; 0 failed", true, 2);
        state.observe_reasoning("The cache is the cause because reads are stale.", 3);
        state.finish();
        state.begin_run(2, "");
        let live: Vec<&str> = state
            .summary()
            .active_decisions()
            .map(|decision| decision.statement.as_str())
            .collect();
        assert!(live.iter().any(|line| line.contains("parser test")));
        assert!(!live.iter().any(|line| line.contains("cache")));
        assert_eq!(state.summary().phase.as_str(), "exploring");

        // A later turn about unrelated work drops even verified decisions that
        // share no subject with the new request.
        state.finish();
        state.begin_run(3, "deploy the release notes");
        assert!(!state
            .summary()
            .active_decisions()
            .any(|decision| decision.statement.contains("parser")));
    }

    #[test]
    fn an_interrupted_turn_resumes_its_live_summary() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "");
        state.observe_reasoning("I will run the parser test because it fails.", 1);
        let snapshot = state.snapshot_cloned();
        let mut resumed = ThinkingState::from_snapshot(Some(snapshot));
        resumed.begin_run(2, "continue the parser work");
        assert!(resumed
            .summary()
            .active_decisions()
            .any(|decision| decision.statement.contains("parser test")));
        assert!(resumed
            .prepare_injection(1, &[])
            .is_some_and(|note| note.content.contains("[source: thinking-summary")));
    }
}
