//! The thinking lifecycle: ingest reasoning and tool outcomes, keep the
//! summary current, and emit one host-guidance note per request.

use super::extract::{
    bounded_tail, extract_decisions, extract_next_action, extract_questions, first_line,
    is_negated, looks_failed, looks_verifying,
};
use super::summary::{
    PendingTool, ThinkingConfidence, ThinkingDecision, ThinkingDecisionKind, ThinkingOpenQuestion,
    ThinkingPhase, ThinkingSnapshot, ThinkingSummary,
};
use super::{
    contains_token, is_cjk, normalize_text, significant_tokens, similarity, ThinkingOrigin,
    THINKING_SOURCE,
};
use crate::llm::{ContextBlock, ContextRole, LlmMessage, ReactSession};

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
    /// A completed previous turn is always a new task: it keeps only the
    /// corroborated decisions and unresolved questions that still overlap
    /// `task`. An unfinished turn is a resume only when the new request still
    /// names the same task (or is a continuation command, or relates to a
    /// pending call); otherwise the host moved on, so the stale pending state
    /// is cleared as well. The turn phase alone cannot make that decision.
    pub fn begin_run(&mut self, turn: u64, task: &str) {
        if self.snapshot.summary.turn == 0 {
            self.snapshot = ThinkingSnapshot::new(turn);
            self.snapshot.task_signature = task_signature(task);
        } else if self.snapshot.summary.phase.is_terminal() || !self.is_same_task(task) {
            self.begin_new_task(turn, task);
        } else {
            self.snapshot.summary.turn = turn;
            if self.snapshot.task_signature.is_none() {
                self.snapshot.task_signature = task_signature(task);
            }
            self.mark_interrupted_pending();
        }
        self.injected = None;
    }

    /// Start a genuinely new task, keeping only the prior state that still
    /// bears on it.
    fn begin_new_task(&mut self, turn: u64, task: &str) {
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
        self.snapshot.pending.clear();
        self.snapshot.consecutive_failures = 0;
        self.snapshot.task_signature = task_signature(task);
    }

    /// Whether the new request belongs to the unfinished task.
    ///
    /// An empty request or a generic continuation cannot prove the opposite, so
    /// it resumes. A different `task` with no word in common with the stored
    /// signature and no relation to a pending call is a new task.
    fn is_same_task(&self, task: &str) -> bool {
        let task = task.trim();
        if task.is_empty() || is_continuation_command(task) {
            return true;
        }
        match &self.snapshot.task_signature {
            Some(signature) if !signature.is_empty() => {
                if tasks_overlap(signature, task) {
                    return true;
                }
            }
            // An older snapshot has no signature; it cannot distinguish, so
            // the historical resume behaviour is preserved.
            _ => return true,
        }
        self.snapshot.pending.iter().any(|pending| {
            pending
                .action
                .as_deref()
                .is_some_and(|action| tasks_overlap(action, task))
                || tasks_overlap(&pending.name, task)
        })
    }

    /// Represent calls that were in flight when the run stopped.
    ///
    /// The process cannot know whether an interrupted call took effect, so the
    /// call stays in the snapshot as `interrupted` and its outcome is recorded
    /// as an open question. It is never treated as completed, and it cannot be
    /// paired with a later result of the same tool name.
    fn mark_interrupted_pending(&mut self) {
        if self.snapshot.pending.is_empty() {
            return;
        }
        let mut changed = false;
        for pending in &mut self.snapshot.pending {
            if pending.interrupted {
                continue;
            }
            pending.interrupted = true;
            let planned = pending
                .action
                .as_deref()
                .map(|action| format!(" (planned: {action})"))
                .unwrap_or_default();
            let question = format!(
                "The {} call at step {} was interrupted before its result; its outcome is unknown{planned}.",
                pending.name, pending.step
            );
            let duplicate = self.snapshot.summary.open_questions.iter().any(|existing| {
                !existing.resolved && similarity(&existing.question, &question) >= 0.8
            });
            if !duplicate {
                self.snapshot
                    .summary
                    .open_questions
                    .push(ThinkingOpenQuestion {
                        question,
                        step: pending.step,
                        resolved: false,
                    });
            }
            changed = true;
        }
        if changed {
            if let Some(pending) = self.snapshot.pending.first() {
                self.snapshot.summary.next_action = Some(format!(
                    "Determine whether the interrupted {} call took effect before retrying",
                    pending.name
                ));
            }
            self.prune();
            self.bump();
        }
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
        // The pending action is what this call is meant to carry out. Prefer the
        // explicit next action; otherwise use the most recent live intent from
        // this step, so a concrete "I will search" is retired when that search
        // returns instead of surviving as stale narration.
        let action = self.snapshot.summary.next_action.clone().or_else(|| {
            self.snapshot
                .summary
                .decisions
                .iter()
                .rev()
                .find(|decision| {
                    decision.is_live()
                        && decision.kind == ThinkingDecisionKind::Intent
                        && decision.step == step
                })
                .map(|decision| decision.statement.clone())
        });
        self.snapshot.pending.push(PendingTool {
            name: name.to_string(),
            step,
            action,
            interrupted: false,
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
    /// A success verifies only a claim the output actually supports. A failure
    /// retires the judgment that led to the call, withdraws a verification the
    /// same tool had supported, adds the open question the failure raises, and
    /// sets the next action to diagnosis instead of a retry.
    pub fn observe_tool_result(&mut self, name: &str, output: &str, success: bool, step: usize) {
        let pending = self.take_pending(name);
        let action = pending.and_then(|pending| pending.action);
        let failed = !success || looks_failed(output);

        if failed {
            self.snapshot.consecutive_failures =
                self.snapshot.consecutive_failures.saturating_add(1);
            let excerpt = first_line(output);
            let output_norm = normalize_text(output);
            for decision in &mut self.snapshot.summary.decisions {
                if !decision.is_live() {
                    continue;
                }
                let related = action
                    .as_deref()
                    .is_some_and(|action| similarity(&decision.statement, action) >= 0.3);
                // A failure of the same tool is only about this claim when the
                // failure text names the claim's subject. An unrelated `bash`
                // failure must not erase a verification from another command.
                let failure_about_claim = significant_tokens(&decision.statement)
                    .iter()
                    .any(|token| contains_token(&output_norm, token));
                let evidenced_by_tool = decision
                    .evidence
                    .iter()
                    .any(|line| line.to_lowercase().contains(&name.to_lowercase()));
                if !related && !(evidenced_by_tool && failure_about_claim) {
                    continue;
                }
                if decision.confidence == ThinkingConfidence::Verified {
                    // The verification rested on this tool. A failure means the
                    // support is gone; the claim returns to unverified rather
                    // than staying a fact.
                    decision.confidence = ThinkingConfidence::Inferred;
                    decision.origin = ThinkingOrigin::Reasoning;
                    decision.evidence.push(format!(
                        "{name} at step {step} failed; verification withdrawn: {excerpt}"
                    ));
                } else {
                    decision.active = false;
                    decision.confidence = ThinkingConfidence::Superseded;
                    decision
                        .evidence
                        .push(format!("{name} failed at step {step}: {excerpt}"));
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
            for decision in &mut self.snapshot.summary.decisions {
                if !decision.is_live()
                    || decision.confidence != ThinkingConfidence::Inferred
                    || !decision.kind.is_verifiable()
                {
                    continue;
                }
                // Success alone is not evidence for the decision: the output
                // must actually support this statement, with matching
                // polarity and content. An intent is never promoted.
                if let Some(excerpt) = confirming_excerpt(output, &decision.statement) {
                    decision.confidence = ThinkingConfidence::Verified;
                    decision.origin = ThinkingOrigin::ToolResult;
                    decision
                        .evidence
                        .push(format!("{name} at step {step} confirmed: {excerpt}"));
                }
            }
            if let Some(action) = action.as_deref() {
                self.retire_intent(action, name, step);
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

    /// Retire the live intent a successful call actually carried out.
    ///
    /// Only an intent whose statement matches the action stored when the call
    /// was issued is retired; another active intent is unrelated and stays
    /// live. A failure never reaches this path.
    fn retire_intent(&mut self, action: &str, name: &str, step: usize) {
        for decision in &mut self.snapshot.summary.decisions {
            if !decision.is_live() || decision.kind != ThinkingDecisionKind::Intent {
                continue;
            }
            if similarity(&decision.statement, action) < 0.5 {
                continue;
            }
            decision.active = false;
            decision.confidence = ThinkingConfidence::Superseded;
            decision.superseded_by = Some(format!("{name} completed at step {step}"));
            decision.evidence.push(format!(
                "{name} succeeded at step {step}; the planned action was carried out"
            ));
        }
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
    ) -> Option<ContextBlock> {
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
        Some(
            ContextBlock::non_authoritative(THINKING_SOURCE, content)
                .with_role(ContextRole::Assistant),
        )
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
        // An interrupted call has no result to pair with it. Only a live call
        // may be resolved by a later result of the same tool name.
        let index = self
            .snapshot
            .pending
            .iter()
            .position(|pending| pending.name == name && !pending.interrupted)?;
        Some(self.snapshot.pending.remove(index))
    }

    fn bump(&mut self) {
        self.snapshot.summary.revision = self.snapshot.summary.revision.saturating_add(1);
    }
}

/// The orientation of a claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Polarity {
    /// Asserts that something holds.
    Positive,
    /// Asserts that something fails, is absent, or does not hold.
    Negative,
    /// Makes no directional assertion.
    Neutral,
}

/// A claim's effective polarity and the predicate root that carries it.
#[derive(Debug, Clone, Copy)]
struct ClaimPolarity {
    polarity: Polarity,
    predicate: Option<&'static str>,
}

/// Predicates that assert failure or absence.
const NEGATIVE_PREDICATES: &[&str] = &[
    "fail",
    "error",
    "broken",
    "missing",
    "absent",
    "crash",
    "panic",
    "unsuccess",
    "失败",
    "错误",
    "缺失",
    "崩溃",
];

/// Predicates that assert success or a fix.
const SUCCESS_PREDICATES: &[&str] = &[
    "pass", "work", "fix", "success", "succeed", "correct", "resolve", "通过", "成功", "修复",
    "正确",
];

/// Predicates that assert presence or evidence.
const PRESENCE_PREDICATES: &[&str] = &[
    "find", "found", "contain", "present", "confirm", "verif", "match",
];

/// All predicates that can carry a positive claim.
const POSITIVE_PREDICATES: &[&str] = &[
    "pass", "work", "fix", "success", "succeed", "correct", "resolve", "find", "found", "contain",
    "present", "confirm", "verif", "match", "通过", "成功", "修复", "正确",
];

/// The first line of `output`, when it actually supports `statement`.
///
/// Success is not evidence. The output must match the claim's effective
/// polarity (a passing result cannot confirm "the test does not pass"), carry
/// the claim's predicate or a direct antonym, and share enough significant
/// tokens to be about the same subject. `Intent` decisions never reach this
/// function.
fn confirming_excerpt(output: &str, statement: &str) -> Option<String> {
    let statement_lower = statement.to_lowercase();
    let output_lower = output.to_lowercase();
    let claim = claim_polarity(&statement_lower);
    let output_polarity = output_polarity(&output_lower);

    let polarity_ok = match claim.polarity {
        Polarity::Positive => output_polarity == Polarity::Positive,
        Polarity::Negative => output_polarity == Polarity::Negative,
        Polarity::Neutral => output_polarity == Polarity::Positive,
    };
    if !polarity_ok || !predicate_supported(&claim, &statement_lower, &output_lower) {
        return None;
    }

    let normalized_output = normalize_text(output);
    let overlaps = significant_tokens(statement)
        .iter()
        .filter(|token| contains_token(&normalized_output, token))
        .count();
    // A test/build marker is itself evidence about the named test; otherwise
    // require two shared significant tokens so a generic word cannot verify a
    // claim on its own.
    let test_evidence = statement_lower.contains("test")
        && (negated_negative_predicate(&output_lower)
            || SUCCESS_PREDICATES
                .iter()
                .any(|root| contains_predicate(&output_lower, root))
            || NEGATIVE_PREDICATES
                .iter()
                .any(|root| contains_predicate(&output_lower, root)));
    let explicit_test_marker = output_lower.contains("test result:")
        || output_lower.contains("tests passed")
        || output_lower.contains("0 failed")
        || test_evidence;
    let needed = if explicit_test_marker { 1 } else { 2 };
    if overlaps < needed {
        return None;
    }
    Some(first_line(output))
}

/// Find a predicate root at a word boundary, accepting common inflections.
///
/// A bare substring search would see `work` inside `network` and `error`
/// inside `terror`; those are not predicates of the sentence.
fn find_predicate(text: &str, root: &str) -> Option<usize> {
    predicate_occurrences(text, root).last().copied()
}

/// Every predicate occurrence, in reading order.
fn predicate_occurrences(text: &str, root: &str) -> Vec<usize> {
    if root.chars().any(is_cjk) {
        return text.match_indices(root).map(|(index, _)| index).collect();
    }
    text.match_indices(root)
        .filter_map(|(index, _)| predicate_bounds(text, index, root).then_some(index))
        .collect()
}

/// Whether `root` at `index` is a whole predicate word.
fn predicate_bounds(text: &str, index: usize, root: &str) -> bool {
    let before_ok = text[..index]
        .chars()
        .next_back()
        .is_none_or(|ch| !ch.is_alphanumeric());
    let rest = &text[index + root.len()..];
    let after_ok = rest.is_empty()
        || rest.chars().next().is_none_or(|ch| !ch.is_alphanumeric())
        || [
            "s", "es", "ed", "d", "ing", "ked", "king", "ure", "ures", "ful", "fully", "ied", "ies",
        ]
        .iter()
        .any(|suffix| {
            rest.strip_prefix(suffix)
                .is_some_and(|tail| tail.chars().next().is_none_or(|ch| !ch.is_alphanumeric()))
        });
    before_ok && after_ok
}

/// Whether a predicate root appears at a word boundary.
fn contains_predicate(text: &str, root: &str) -> bool {
    find_predicate(text, root).is_some()
}

/// Resolve a claim's effective polarity.
///
/// The predicate decides, and a negation immediately before it inverts the
/// result: "does not fail" is a positive claim, "does not pass" is a negative
/// one. A sentence with no predicate but a negation ("is not the cause") is an
/// absence claim, not a positive one. Positive predicates win over a failure
/// noun in the object ("the failure is fixed").
fn claim_polarity(statement: &str) -> ClaimPolarity {
    let lower = statement.replace("not only", "only");
    let mut negated: Option<(&'static str, usize)> = None;
    let mut positive: Option<(&'static str, usize)> = None;
    let mut negative: Option<(&'static str, usize)> = None;
    for (root, is_negative) in NEGATIVE_PREDICATES
        .iter()
        .map(|root| (*root, true))
        .chain(POSITIVE_PREDICATES.iter().map(|root| (*root, false)))
    {
        let Some(index) = find_predicate(&lower, root) else {
            continue;
        };
        if negated_before(&lower, index) {
            if negated.is_none_or(|(_, at)| index > at) {
                negated = Some((root, index));
            }
        } else if is_negative {
            if negative.is_none_or(|(_, at)| index > at) {
                negative = Some((root, index));
            }
        } else if positive.is_none_or(|(_, at)| index > at) {
            positive = Some((root, index));
        }
    }
    if let Some((root, _)) = negated {
        let base_negative = NEGATIVE_PREDICATES.contains(&root);
        return ClaimPolarity {
            polarity: if base_negative {
                Polarity::Positive
            } else {
                Polarity::Negative
            },
            predicate: Some(root),
        };
    }
    if let Some((root, _)) = positive {
        return ClaimPolarity {
            polarity: Polarity::Positive,
            predicate: Some(root),
        };
    }
    if let Some((root, _)) = negative {
        return ClaimPolarity {
            polarity: Polarity::Negative,
            predicate: Some(root),
        };
    }
    ClaimPolarity {
        polarity: if has_negation(&lower) {
            Polarity::Negative
        } else {
            Polarity::Neutral
        },
        predicate: None,
    }
}

/// Whether a negation appears in the short window before `index`.
fn negated_before(text: &str, index: usize) -> bool {
    let prefix = &text[..index];
    let tail: String = prefix
        .chars()
        .rev()
        .take(48)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if tail.contains("n't")
        || tail.contains("fails to ")
        || tail.contains("failed to ")
        || tail.contains("unable to ")
        || [
            "not", "never", "no", "none", "neither", "zero", "cannot", "without",
        ]
        .iter()
        .any(|marker| contains_token(&tail, marker))
    {
        return true;
    }
    ["不", "没有", "未", "并非", "无"]
        .iter()
        .any(|marker| tail.contains(marker))
}

/// Whether the statement contains an explicit negation at all.
fn has_negation(text: &str) -> bool {
    if text.contains("n't")
        || [
            "not", "never", "no", "none", "neither", "zero", "cannot", "without",
        ]
        .iter()
        .any(|marker| contains_token(text, marker))
    {
        return true;
    }
    ["不", "没有", "未", "并非", "无"]
        .iter()
        .any(|marker| text.contains(marker))
}

/// Whether the claim's predicate (or its antonym) is present in the output.
fn predicate_supported(claim: &ClaimPolarity, statement: &str, output: &str) -> bool {
    let Some(root) = claim.predicate else {
        // An absence claim has no predicate; polarity and shared tokens carry
        // the decision.
        return true;
    };
    if contains_predicate(output, root) || presence_synonym(root, output) {
        return true;
    }
    // A negative presence claim ("not present") is confirmed by an absence
    // report even when the report uses a different presence verb.
    if claim.polarity == Polarity::Negative
        && PRESENCE_PREDICATES.contains(&root)
        && output_is_absence(output)
    {
        return true;
    }
    // An absence report ("no test named parser") is not failure evidence for a
    // claim about a test failing; only the claim's own predicate may confirm it.
    if output_is_absence(output) && !PRESENCE_PREDICATES.contains(&root) {
        return false;
    }
    // "The test did not fail" confirms a passing-test claim: a negated failure
    // predicate is positive evidence even though it is not the word "pass".
    if claim.polarity == Polarity::Positive
        && statement.contains("test")
        && negated_negative_predicate(output)
    {
        return true;
    }
    let family: &[&str] = if claim.polarity == Polarity::Negative {
        NEGATIVE_PREDICATES
    } else if NEGATIVE_PREDICATES.contains(&root) {
        // A negated negative predicate ("does not fail") is a positive claim.
        SUCCESS_PREDICATES
    } else if PRESENCE_PREDICATES.contains(&root) {
        PRESENCE_PREDICATES
    } else {
        SUCCESS_PREDICATES
    };
    if family
        .iter()
        .any(|marker| contains_predicate(output, marker))
    {
        return true;
    }
    let explicit_test_marker = output.contains("test result:")
        || output.contains("tests passed")
        || output.contains("0 failed");
    explicit_test_marker && statement.contains("test")
}

/// Whether the output negates a failure predicate ("did not fail", "no errors").
fn negated_negative_predicate(output: &str) -> bool {
    NEGATIVE_PREDICATES.iter().any(|root| {
        predicate_occurrences(output, root)
            .iter()
            .any(|index| negated_before(output, *index))
    })
}

/// Whether a presence root has an equivalent form in the output.
fn presence_synonym(root: &str, output: &str) -> bool {
    match root {
        "find" => contains_predicate(output, "found"),
        "found" => contains_predicate(output, "find"),
        _ => false,
    }
}

/// Whether the output reports an absence rather than a failure.
fn output_is_absence(output: &str) -> bool {
    output.contains("not found")
        || output.contains("no test")
        || ["absent", "missing", "none"]
            .iter()
            .any(|marker| contains_token(output, marker))
}

/// Classify what an output reports, including the negation around its predicates.
///
/// A score is used rather than a single marker: an output can say "test result:
/// ok. 4 passed; 0 failed" (positive) or "the test did not fail because no
/// errors were found" (also positive), and a single substring cannot tell those
/// from "the test did not pass" (negative). Negation immediately before a
/// predicate flips its contribution.
fn output_polarity(output: &str) -> Polarity {
    let lower = output.to_lowercase().replace("not only", "only");
    let mut score = 0i32;
    let mut evidence = false;

    if lower.contains("test result: ok") {
        score += 4;
        evidence = true;
    }
    if lower.contains("test result: failed") {
        score -= 4;
        evidence = true;
    }
    for marker in [
        "0 failed",
        "0 failures",
        "0 errors",
        "no errors",
        "no failures",
    ] {
        if lower.contains(marker) {
            score += 3;
            evidence = true;
        }
    }
    if lower.contains("\"success\": false") {
        score -= 3;
        evidence = true;
    }
    if lower.contains("no test") || lower.contains("not found") {
        score -= 2;
        evidence = true;
    }

    for (root, base) in NEGATIVE_PREDICATES
        .iter()
        .map(|root| (*root, -2))
        .chain(POSITIVE_PREDICATES.iter().map(|root| (*root, 2)))
    {
        for index in predicate_occurrences(&lower, root) {
            let negated = negated_before(&lower, index);
            score += if negated { -base } else { base };
            evidence = true;
        }
    }

    if !evidence {
        return Polarity::Neutral;
    }
    if score > 0 {
        Polarity::Positive
    } else if score < 0 {
        Polarity::Negative
    } else {
        Polarity::Neutral
    }
}

/// A bounded signature of the request that opened a turn.
///
/// Only significant words are kept, deduplicated and capped at 24 tokens. The
/// signature is a lifecycle key for resume-versus-new-task decisions; the raw
/// request body is not stored.
fn task_signature(task: &str) -> Option<String> {
    let tokens = significant_tokens(task);
    let mut seen = std::collections::BTreeSet::new();
    let signature: Vec<&str> = tokens
        .iter()
        .filter(|token| seen.insert(token.as_str()))
        .take(24)
        .map(String::as_str)
        .collect();
    (!signature.is_empty()).then(|| signature.join(" "))
}

/// Whether the request is a generic instruction to continue the prior work.
fn is_continuation_command(task: &str) -> bool {
    let lower = normalize_text(task);
    if lower.is_empty() {
        return true;
    }
    [
        "continue",
        "resume",
        "go on",
        "keep going",
        "carry on",
        "proceed",
    ]
    .iter()
    .any(|cue| {
        lower
            .strip_prefix(cue)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
    }) || ["继续", "接着", "往下"]
        .iter()
        .any(|cue| lower.starts_with(cue))
}

/// Verbs that say what to do but not what the work is about.
const GENERIC_TASK_WORDS: &[&str] = &[
    "fix", "run", "test", "check", "add", "update", "use", "make", "get", "try", "work", "need",
    "should", "build", "write", "read", "change", "remove",
];

/// Whether two requests are about the same work.
///
/// One shared word is not enough: "fix the parser" and "fix the deployment"
/// share only the generic verb. A resume needs either two shared significant
/// words with meaningful overlap, or a single shared word when both requests
/// are very short.
fn tasks_overlap(left: &str, right: &str) -> bool {
    let left_tokens = significant_tokens(left);
    let right_tokens = significant_tokens(right);
    if left_tokens.is_empty() || right_tokens.is_empty() {
        return false;
    }
    let left_set: std::collections::BTreeSet<&str> =
        left_tokens.iter().map(String::as_str).collect();
    let right_set: std::collections::BTreeSet<&str> =
        right_tokens.iter().map(String::as_str).collect();
    if left_set == right_set {
        return true;
    }
    let shared_tokens: Vec<&str> = left_set.intersection(&right_set).copied().collect();
    let shared = shared_tokens.len();
    if shared == 0 {
        return false;
    }
    if shared >= 2 {
        let union = left_set.union(&right_set).count();
        return shared as f32 / union as f32 >= 0.25;
    }
    // A single-word request can continue the work when its word is the
    // subject, not a generic verb such as "fix" or "run".
    right_set.len() <= 1
        && !shared_tokens
            .first()
            .is_some_and(|token| GENERIC_TASK_WORDS.contains(token))
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

/// Whether two tokens name the same word across a simple inflection.
fn token_matches(left: &str, right: &str) -> bool {
    left == right
        || (left.len() >= 4
            && right.len() >= 4
            && left.len().abs_diff(right.len()) <= 2
            && (left.starts_with(right) || right.starts_with(left)))
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
                Some((content.as_str(), true))
            }
            // Tool output is evidence, not narration. Exact text still counts
            // as a duplicate, but a paraphrase check against a large result
            // would suppress a claim merely because its subject words appear
            // in the output.
            LlmMessage::ToolResult { content, .. } => Some((content.as_str(), false)),
            LlmMessage::User { content } => content.as_text().map(|text| (text, true)),
            // Reasoning is the source the summary is distilled from, not a
            // history turn the reader or the assistant said. Counting it here
            // would make every summary item look duplicated.
            LlmMessage::Developer { .. }
            | LlmMessage::AssistantReasoning { .. }
            | LlmMessage::AssistantToolCall { .. } => None,
        })
        .any(|(body, narration)| {
            let normalized = normalize_text(body);
            if normalized.contains(&needle) || normalized.contains(&prefix) {
                return true;
            }
            if !narration {
                return false;
            }
            // A paraphrase of a carried item is already represented even when
            // the exact words differ. Compare the whole row and its first lines
            // with bounded work, so the duplicate guard does not depend on a
            // substring hit alone (issue #441).
            if similarity(&normalized, &needle) >= 0.8 {
                return true;
            }
            // A paraphrase may use a plural or inflection (fixture/fixtures).
            // If every significant item token is covered by the row, the item
            // is already represented even when Jaccard is below the threshold.
            let item_tokens = significant_tokens(&needle);
            if !item_tokens.is_empty() {
                let body_tokens = significant_tokens(&normalized);
                if item_tokens
                    .iter()
                    .all(|token| body_tokens.iter().any(|body| token_matches(token, body)))
                {
                    return true;
                }
            }
            body.lines().take(24).any(|line| {
                let line = normalize_text(line);
                line.chars().count() >= 12
                    && (line.contains(&prefix) || similarity(&line, &needle) >= 0.7)
            })
        })
}

#[cfg(test)]
mod tests {
    use super::{claim_polarity, output_polarity, Polarity, ThinkingState};
    use crate::llm::LlmMessage;
    use crate::thinking::summary::ThinkingConfidence;

    #[test]
    fn complex_output_negation_is_classified() {
        assert_eq!(output_polarity("the test did not fail"), Polarity::Positive);
        assert_eq!(output_polarity("the test did not pass"), Polarity::Negative);
        assert_eq!(
            output_polarity("the test is not failing"),
            Polarity::Positive
        );
        assert_eq!(
            output_polarity("the test is not passing"),
            Polarity::Negative
        );
        assert_eq!(
            output_polarity("the test failed to pass"),
            Polarity::Negative
        );
        assert_eq!(output_polarity("no bug was found"), Polarity::Negative);
        assert_eq!(
            output_polarity("the bug is not present"),
            Polarity::Negative
        );
        assert_eq!(
            output_polarity("no failures were found"),
            Polarity::Positive
        );
        assert_eq!(output_polarity("0 failures"), Polarity::Positive);
        assert_eq!(output_polarity("no errors"), Polarity::Positive);
        assert_eq!(output_polarity("unsuccessful"), Polarity::Negative);
        assert_eq!(output_polarity("the build succeeded"), Polarity::Positive);
        assert_eq!(
            output_polarity("the issue was resolved"),
            Polarity::Positive
        );
        assert_eq!(
            output_polarity("the parser found no failures"),
            Polarity::Positive
        );
        assert_eq!(
            output_polarity("none of the tests failed"),
            Polarity::Positive
        );
        assert_eq!(output_polarity("zero failures"), Polarity::Positive);
        assert_eq!(
            output_polarity("none of the tests passed"),
            Polarity::Negative
        );
        assert_eq!(
            output_polarity("after the full rebuild and a clean install there were no failures"),
            Polarity::Positive
        );
    }

    #[test]
    fn a_passing_test_is_confirmed_by_did_not_fail() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "");
        state.observe_reasoning(
            "The issue is that the test passes because the fixture exists.",
            1,
        );
        state.observe_tool_call("bash", 2);
        state.observe_tool_result("bash", "the test did not fail", true, 2);
        assert_eq!(
            state.summary().decisions[0].confidence,
            ThinkingConfidence::Verified,
            "a negated failure confirms a passing claim"
        );
    }

    #[test]
    fn a_passing_test_confirms_does_not_fail() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "");
        state.observe_reasoning(
            "The issue is that the test does not fail because the fix is correct.",
            1,
        );
        state.observe_tool_call("bash", 2);
        state.observe_tool_result("bash", "the test passed", true, 2);
        assert_eq!(
            state.summary().decisions[0].confidence,
            ThinkingConfidence::Verified
        );
    }

    #[test]
    fn a_failing_test_confirms_does_not_pass() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "");
        state.observe_reasoning(
            "The issue is that the test does not pass because the fixture is missing.",
            1,
        );
        state.observe_tool_call("bash", 2);
        state.observe_tool_result("bash", "the test failed", true, 2);
        assert_eq!(
            state.summary().decisions[0].confidence,
            ThinkingConfidence::Verified
        );
    }

    #[test]
    fn a_failing_test_is_not_confirmed_by_did_not_fail() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "");
        state.observe_reasoning(
            "The issue is that the parser test fails because the fixture is missing.",
            1,
        );
        state.observe_tool_call("bash", 2);
        state.observe_tool_result("bash", "the parser test did not fail", true, 2);
        assert_eq!(
            state.summary().decisions[0].confidence,
            ThinkingConfidence::Inferred,
            "a negated failure contradicts a failing claim"
        );
    }

    #[test]
    fn a_negative_presence_claim_is_confirmed_by_absence() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "");
        state.observe_reasoning(
            "The bug is not present in the parser because the file was rebuilt.",
            1,
        );
        state.observe_tool_call("bash", 2);
        state.observe_tool_result("bash", "the parser bug was not found", true, 2);
        assert_eq!(
            state.summary().decisions[0].confidence,
            ThinkingConfidence::Verified,
            "absence evidence confirms a negative presence claim"
        );
    }

    #[test]
    fn predicate_matching_ignores_words_that_merely_contain_it() {
        assert_eq!(
            claim_polarity("the network works").polarity,
            Polarity::Positive
        );
        assert_eq!(
            claim_polarity("the network fails").polarity,
            Polarity::Negative
        );
    }

    #[test]
    fn a_success_without_confirming_evidence_leaves_the_claim_inferred() {
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
            "tool success alone does not turn a claim into a fact"
        );
        assert!(
            decision.evidence.is_empty(),
            "execution success is not recorded as evidence for the claim"
        );
    }

    #[test]
    fn confirming_evidence_promotes_only_the_supported_claim() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "");
        state.observe_reasoning(
            "The bug is in the parser file because the failure starts there. The cause is the cache because reads are stale. I will run the parser test.",
            1,
        );
        state.observe_tool_call("bash", 2);
        state.observe_tool_result(
            "bash",
            "grep found the bug in parser.rs; test result: ok. 4 passed; 0 failed",
            true,
            2,
        );

        let bug = state
            .summary()
            .decisions
            .iter()
            .find(|decision| decision.statement.contains("bug"))
            .expect("the supported claim");
        assert_eq!(bug.confidence, ThinkingConfidence::Verified);
        assert!(
            bug.evidence.iter().any(|line| line.contains("confirmed")),
            "verification records the supporting evidence: {:?}",
            bug.evidence
        );

        let cache = state
            .summary()
            .decisions
            .iter()
            .find(|decision| decision.statement.contains("cache"))
            .expect("the other claim");
        assert_eq!(cache.confidence, ThinkingConfidence::Inferred);

        let intent = state
            .summary()
            .decisions
            .iter()
            .find(|decision| decision.statement.contains("I will run"))
            .expect("the intended action");
        assert_eq!(
            intent.confidence,
            ThinkingConfidence::Superseded,
            "a completed intent is retired, not verified"
        );
        assert!(!intent.active, "a completed intent is not injected");
    }

    #[test]
    fn a_successful_call_retires_the_intent_it_ran_and_keeps_another() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "");
        state.observe_reasoning(
            "I will update the README. I will search the parser fixture.",
            1,
        );
        state.observe_tool_call("bash", 1);
        state.observe_tool_result("bash", "", true, 2);

        let search = state
            .summary()
            .decisions
            .iter()
            .find(|decision| decision.statement.to_lowercase().contains("search"))
            .expect("the completed search intent");
        assert!(
            !search.is_live(),
            "the action the call carried out is retired: {search:?}"
        );
        assert_eq!(search.confidence, ThinkingConfidence::Superseded);

        let readme = state
            .summary()
            .decisions
            .iter()
            .find(|decision| decision.statement.contains("README"))
            .expect("the unrelated intent");
        assert!(
            readme.is_live(),
            "an unrelated successful call keeps the other intent live: {readme:?}"
        );

        let note = state
            .prepare_injection(3, &[])
            .expect("the still-pending intent is injected");
        assert!(
            note.content.contains("README"),
            "the pending intent remains: {}",
            note.content
        );
        assert!(
            !note.content.to_lowercase().contains("search the parser"),
            "the completed search is gone: {}",
            note.content
        );
    }

    #[test]
    fn a_completed_search_is_not_injected_on_the_next_step() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "");
        state.observe_reasoning("I will search the workspace for relevant files.", 1);
        state.observe_tool_call("bash", 1);
        state.observe_tool_result("bash", "", true, 2);
        assert!(
            state.prepare_injection(3, &[]).is_none(),
            "the completed search no longer contributes a note"
        );
    }

    #[test]
    fn a_paraphrased_item_already_in_history_is_not_injected() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "");
        state.observe_reasoning("I will search the parser fixture.", 1);
        let history = vec![LlmMessage::assistant(
            "I will search the parser fixtures before editing.",
        )];
        let note = state.prepare_injection(2, &history);
        assert!(
            note.as_ref()
                .is_none_or(|note| !note.content.to_lowercase().contains("search the parser")),
            "a paraphrase already in history is not repeated: {note:?}"
        );
    }

    #[test]
    fn a_negated_claim_is_not_confirmed_by_positive_evidence() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "");
        state.observe_reasoning(
            "The bug is not in the parser because the failure starts elsewhere.",
            1,
        );
        state.observe_tool_call("bash", 2);
        state.observe_tool_result("bash", "grep found the bug in parser.rs", true, 2);
        let decision = state
            .summary()
            .decisions
            .iter()
            .find(|decision| decision.statement.contains("bug"))
            .expect("the negated claim");
        assert_eq!(
            decision.confidence,
            ThinkingConfidence::Inferred,
            "positive evidence cannot confirm an absence claim"
        );
    }

    #[test]
    fn a_negated_claim_is_confirmed_by_absence_evidence() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "");
        state.observe_reasoning(
            "The bug is not in the parser because the failure starts elsewhere.",
            1,
        );
        state.observe_tool_call("bash", 2);
        state.observe_tool_result("bash", "bug not found in parser", true, 2);
        let decision = state
            .summary()
            .decisions
            .iter()
            .find(|decision| decision.statement.contains("bug"))
            .expect("the negated claim");
        assert_eq!(decision.confidence, ThinkingConfidence::Verified);
    }

    #[test]
    fn a_negated_positive_predicate_requires_failure_evidence() {
        let mut passing = ThinkingState::new();
        passing.begin_run(1, "");
        passing.observe_reasoning(
            "The issue is that the parser test does not pass because the fixture is missing.",
            1,
        );
        passing.observe_tool_call("bash", 2);
        passing.observe_tool_result("bash", "test result: ok. parser test passed", true, 2);
        assert_eq!(
            passing.summary().decisions[0].confidence,
            ThinkingConfidence::Inferred,
            "a passing result contradicts 'does not pass'"
        );

        let mut failing = ThinkingState::new();
        failing.begin_run(1, "");
        failing.observe_reasoning(
            "The issue is that the parser test does not pass because the fixture is missing.",
            1,
        );
        failing.observe_tool_call("bash", 2);
        failing.observe_tool_result("bash", "the parser test failed", true, 2);
        assert_eq!(
            failing.summary().decisions[0].confidence,
            ThinkingConfidence::Verified,
            "failure evidence confirms 'does not pass'"
        );
    }

    #[test]
    fn fails_to_pass_reads_as_a_negative_claim() {
        let mut passing = ThinkingState::new();
        passing.begin_run(1, "");
        passing.observe_reasoning(
            "The issue is that the parser test fails to pass because the fixture is missing.",
            1,
        );
        passing.observe_tool_call("bash", 2);
        passing.observe_tool_result("bash", "test result: ok. parser test passed", true, 2);
        assert_eq!(
            passing.summary().decisions[0].confidence,
            ThinkingConfidence::Inferred,
            "a pass contradicts 'fails to pass'"
        );

        let mut failing = ThinkingState::new();
        failing.begin_run(1, "");
        failing.observe_reasoning(
            "The issue is that the parser test fails to pass because the fixture is missing.",
            1,
        );
        failing.observe_tool_call("bash", 2);
        failing.observe_tool_result("bash", "the parser test failed", true, 2);
        assert_eq!(
            failing.summary().decisions[0].confidence,
            ThinkingConfidence::Verified,
            "failure evidence confirms 'fails to pass'"
        );
    }

    #[test]
    fn a_negated_negative_predicate_is_a_positive_claim() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "");
        state.observe_reasoning(
            "The issue is that the parser test does not fail because the fix is correct.",
            1,
        );
        state.observe_tool_call("bash", 2);
        state.observe_tool_result("bash", "test result: ok. parser test passed", true, 2);
        assert_eq!(
            state.summary().decisions[0].confidence,
            ThinkingConfidence::Verified,
            "'does not fail' is a positive claim confirmed by a pass"
        );
    }

    #[test]
    fn a_failed_result_withdraws_a_verification_that_rested_on_the_same_tool() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "");
        state.observe_reasoning(
            "The bug is in the parser file because the failure starts there.",
            1,
        );
        state.observe_tool_call("bash", 2);
        state.observe_tool_result("bash", "grep found the bug in parser.rs", true, 2);
        assert_eq!(
            state.summary().decisions[0].confidence,
            ThinkingConfidence::Verified
        );

        state.observe_tool_call("bash", 3);
        state.observe_tool_result("bash", "Error: parser.rs is missing", false, 3);
        assert_eq!(
            state.summary().decisions[0].confidence,
            ThinkingConfidence::Inferred,
            "a failed call withdraws the verification it had supported"
        );
        assert!(state.summary().decisions[0]
            .evidence
            .iter()
            .any(|line| line.contains("withdrawn")));
    }

    #[test]
    fn an_unrelated_failure_does_not_erase_a_verification() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "");
        state.observe_reasoning(
            "The bug is in the parser file because the failure starts there.",
            1,
        );
        state.observe_tool_call("bash", 2);
        state.observe_tool_result("bash", "grep found the bug in parser.rs", true, 2);
        state.observe_tool_call("bash", 3);
        state.observe_tool_result("bash", "Permission denied writing output", false, 3);
        assert_eq!(
            state.summary().decisions[0].confidence,
            ThinkingConfidence::Verified,
            "a failure about a different subject does not erase support"
        );
    }

    #[test]
    fn a_resumed_pending_call_is_preserved_as_unknown() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "");
        state.observe_reasoning(
            "I will run the parser test because it reproduces the failure.",
            1,
        );
        state.observe_tool_call("bash", 2);
        let snapshot = state.snapshot_cloned();

        let mut resumed = ThinkingState::from_snapshot(Some(snapshot));
        resumed.begin_run(2, "continue the parser work");
        assert_eq!(resumed.snapshot.pending.len(), 1, "the call is preserved");
        assert!(resumed.snapshot.pending[0].interrupted);
        assert!(resumed.snapshot.pending[0].action.is_some());
        assert!(resumed
            .summary()
            .open_questions
            .iter()
            .any(|question| question.question.contains("interrupted")
                && question.question.contains("outcome is unknown")));
        assert!(
            !resumed
                .summary()
                .decisions
                .iter()
                .any(|decision| decision.confidence == ThinkingConfidence::Verified),
            "an interrupted call must not be read as success"
        );
    }

    #[test]
    fn a_different_task_after_an_interruption_starts_fresh() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "fix the parser failure");
        state.observe_reasoning(
            "I will run the parser test because it reproduces the failure.",
            1,
        );
        state.observe_tool_call("bash", 2);

        state.begin_run(2, "deploy the release notes");
        assert!(
            state.snapshot.pending.is_empty(),
            "stale pending is cleared"
        );
        assert!(!state
            .summary()
            .open_questions
            .iter()
            .any(|question| question.question.contains("interrupted")));
        assert_eq!(
            state.summary().phase,
            crate::thinking::ThinkingPhase::Exploring
        );
        assert!(state
            .snapshot
            .task_signature
            .as_deref()
            .is_some_and(|signature| signature.contains("deploy")));
        assert!(!state
            .summary()
            .active_decisions()
            .any(|decision| decision.statement.contains("parser")));
    }

    #[test]
    fn a_single_subject_word_resumes_but_a_generic_verb_does_not() {
        let mut by_subject = ThinkingState::new();
        by_subject.begin_run(1, "fix the parser failure");
        by_subject.observe_tool_call("bash", 2);
        by_subject.begin_run(2, "parser");
        assert_eq!(by_subject.snapshot.pending.len(), 1);

        let mut by_verb = ThinkingState::new();
        by_verb.begin_run(1, "fix the parser failure");
        by_verb.observe_tool_call("bash", 2);
        by_verb.begin_run(2, "fix");
        assert!(by_verb.snapshot.pending.is_empty());
    }

    #[test]
    fn a_different_task_with_a_shared_verb_starts_fresh() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "fix the parser failure");
        state.observe_reasoning(
            "I will run the parser test because it reproduces the failure.",
            1,
        );
        state.observe_tool_call("bash", 2);

        state.begin_run(2, "fix the deployment pipeline");
        assert!(
            state.snapshot.pending.is_empty(),
            "a shared generic verb is not the same task"
        );
        assert!(!state
            .summary()
            .open_questions
            .iter()
            .any(|question| question.question.contains("interrupted")));
    }

    #[test]
    fn a_matching_task_after_an_interruption_resumes_pending() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "fix the parser failure");
        state.observe_reasoning(
            "I will run the parser test because it reproduces the failure.",
            1,
        );
        state.observe_tool_call("bash", 2);

        state.begin_run(2, "fix the parser failure again");
        assert_eq!(state.snapshot.pending.len(), 1);
        assert!(state.snapshot.pending[0].interrupted);
        assert!(state
            .summary()
            .open_questions
            .iter()
            .any(|question| question.question.contains("interrupted")));
    }

    #[test]
    fn a_continuation_command_resumes_pending() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "fix the parser failure");
        state.observe_reasoning(
            "I will run the parser test because it reproduces the failure.",
            1,
        );
        state.observe_tool_call("bash", 2);

        state.begin_run(2, "continue");
        assert_eq!(state.snapshot.pending.len(), 1);
        assert!(state.snapshot.pending[0].interrupted);
    }

    #[test]
    fn a_new_task_clears_stale_pending_calls() {
        let mut state = ThinkingState::new();
        state.begin_run(1, "");
        state.observe_tool_call("bash", 2);
        state.snapshot.summary.phase = crate::thinking::ThinkingPhase::Complete;
        state.begin_run(2, "deploy the release notes");
        assert!(state.snapshot.pending.is_empty());
        assert!(!state
            .summary()
            .open_questions
            .iter()
            .any(|question| question.question.contains("interrupted")));
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
        state.observe_reasoning(
            "The bug is in the parser file because the failure starts there. I will run the parser test.",
            1,
        );
        state.observe_tool_call("bash", 2);
        state.observe_tool_result("bash", "grep found the bug in parser.rs", true, 2);
        state.observe_reasoning("The cache is the cause because reads are stale.", 3);
        state.finish();
        state.begin_run(2, "");
        let live: Vec<&str> = state
            .summary()
            .active_decisions()
            .map(|decision| decision.statement.as_str())
            .collect();
        assert!(live.iter().any(|line| line.contains("bug")));
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
        assert!(
            resumed
                .summary()
                .next_action
                .as_deref()
                .is_some_and(|action| action.contains("parser")),
            "an interruption before the tool call keeps the planned action"
        );
        assert!(resumed
            .prepare_injection(1, &[])
            .is_some_and(|note| note.content.contains("[source: thinking-summary")));
    }
}
