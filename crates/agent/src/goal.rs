//! A durable working goal: the objective a session is pursuing, how many rounds
//! it has taken, and why it stopped.
//!
//! The plan (crate::tools::PlanStore) answers "what steps are left in this
//! task"; a goal answers the larger question "what is all of this for", and it
//! outlives any single turn. Hosts render it and drive its transitions, but the
//! model lives here so every host agrees on what a blocked goal means and no UI
//! has to re-implement a state machine.
//!
//! The shape and the transition rules are the DeepSeek Harness goal domain's
//! (@deepseek-ai/dsh-goal, measured from the shipped bundle, round 126): four
//! durable phases plus a process-local activation flag, one current goal per
//! session, a positive round cap (256 by default), and a blocked reason that is
//! a stable lower-kebab-case code plus one explanation. Activation is never
//! persisted: the reference disarms every goal at an agent/created edge, so a
//! goal loaded from disk starts disarmed and only an explicit resume arms it.

use serde::{Deserialize, Deserializer, Serialize};

/// Cap on an objective's length, in characters.
pub const MAX_OBJECTIVE: usize = 2000;
/// Cap on a blocked reason's explanation, in characters.
pub const MAX_REASON: usize = 500;

/// The reference's defaultMaxGoalRounds: the round cap a create takes when it
/// names none. The reference exposes it as deployment configuration; bsh has no
/// user-facing field for it, so every goal created here takes this value.
pub const DEFAULT_MAX_ROUNDS: u32 = 256;

/// The stable code the reference records for a model-reported blocker.
pub const MODEL_REPORTED: &str = "model-reported";

/// Where a goal stands.
///
/// The distinction that matters is between a goal that is *stopped* and one that
/// is *settled*: a paused, blocked or active goal can be resumed, while a
/// completed one has reached a state only a new goal can move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalState {
    /// Being pursued; rounds may advance while continuation is armed.
    #[default]
    Active,
    /// Deliberately stopped, resumable.
    Paused,
    /// The objective is achieved.
    Completed,
    /// Stopped by a concrete condition that must change before progress is
    /// possible again.
    Blocked,
}

impl GoalState {
    /// Whether another round may be counted from this state.
    pub fn is_open(self) -> bool {
        matches!(self, GoalState::Active)
    }

    /// Whether the goal has reached a state nothing continues it from.
    pub fn is_settled(self) -> bool {
        matches!(self, GoalState::Completed | GoalState::Blocked)
    }
}

/// Whether automatic continuation is armed.
///
/// This is the reference's process-local flag, not a durable phase: it is never
/// serialized, and a goal that comes back from a session file starts
/// GoalActivation::Disarmed exactly as it does after the reference's
/// agent/created edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalActivation {
    /// A later round may continue the goal.
    #[default]
    Disarmed,
    /// Continuation is allowed.
    Armed,
}

impl GoalActivation {
    /// The word the reference's /goal report prints for this flag.
    pub fn as_str(self) -> &'static str {
        match self {
            GoalActivation::Armed => "armed",
            GoalActivation::Disarmed => "disarmed",
        }
    }

    /// Whether continuation is allowed.
    pub fn is_armed(self) -> bool {
        matches!(self, GoalActivation::Armed)
    }
}

/// Why a blocked goal stopped: a stable policy code plus the explanation.
///
/// The reference requires a lower-kebab-case code and a non-empty message, and
/// renders "Blocker: code: message". A goal cannot be blocked without both,
/// because a blocker nobody can route on is not a fact anyone can act on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockedReason {
    /// The policy-owned lower-kebab-case code.
    pub code: String,
    /// The human-readable explanation.
    pub message: String,
}

impl BlockedReason {
    /// Build a validated blocker, or None when the code or message is invalid.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Option<Self> {
        let code = code.into();
        if !valid_code(&code) {
            return None;
        }
        let mut message = message.into().trim().to_string();
        if message.is_empty() {
            return None;
        }
        if message.chars().count() > MAX_REASON {
            message = message.chars().take(MAX_REASON).collect();
        }
        Some(Self { code, message })
    }
}

/// Whether one string is the reference's lower-kebab-case policy code.
fn valid_code(code: &str) -> bool {
    if code.is_empty() {
        return false;
    }
    let mut chars = code.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    let mut after_dash = false;
    for character in chars {
        match character {
            '-' => {
                if after_dash {
                    return false;
                }
                after_dash = true;
            }
            'a'..='z' | '0'..='9' => after_dash = false,
            _ => return false,
        }
    }
    !after_dash
}

/// The goal a session is pursuing.
///
/// Every field is sanitized on the way in (see Goal::sanitize), so a host can
/// hand the store whatever a person typed and the store still holds a value
/// every other host can render.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Goal {
    /// What the work is for, as one statement.
    pub objective: String,
    /// Where the goal stands.
    #[serde(default)]
    pub state: GoalState,
    /// Rounds spent pursuing it.
    #[serde(default)]
    pub rounds: u32,
    /// Cap on rounds. The reference always has one (256 by default), so this is
    /// not optional; older files without the field take DEFAULT_MAX_ROUNDS.
    #[serde(
        default = "default_max_rounds",
        deserialize_with = "deserialize_max_rounds"
    )]
    pub max_rounds: u32,
    /// Why a blocked goal stopped, kept only while it is blocked.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_blocked_reason"
    )]
    pub blocked_reason: Option<BlockedReason>,
    /// Whether automatic continuation is armed. Process-local: never serialized,
    /// so a durable reload always comes back disarmed.
    #[serde(skip)]
    pub activation: GoalActivation,
}

/// The cap a goal takes when its file names none.
fn default_max_rounds() -> u32 {
    DEFAULT_MAX_ROUNDS
}

/// Read a round cap that may be absent, null or zero in an older file.
fn deserialize_max_rounds<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    let cap = Option::<u32>::deserialize(deserializer)?;
    Ok(cap.filter(|cap| *cap >= 1).unwrap_or(DEFAULT_MAX_ROUNDS))
}

/// Read a blocked reason that may be the pre-code plain string in an older file.
fn deserialize_blocked_reason<'de, D>(deserializer: D) -> Result<Option<BlockedReason>, D::Error>
where
    D: Deserializer<'de>,
{
    /// The two shapes a stored blocked reason has had.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Stored {
        /// The current code plus message.
        Reason { code: String, message: String },
        /// A pre-code goal, which recorded only the explanation.
        Legacy(String),
    }
    let stored = Option::<Stored>::deserialize(deserializer)?;
    Ok(match stored {
        None => None,
        Some(Stored::Reason { code, message }) => BlockedReason::new(code, message),
        Some(Stored::Legacy(message)) => BlockedReason::new(MODEL_REPORTED, message),
    })
}

impl Goal {
    /// Start a fresh, active goal. None when the objective is blank once
    /// trimmed - an empty goal is the absence of a goal, not a goal with no
    /// objective.
    ///
    /// A created goal is armed, which is what `/goal <objective>` means in the
    /// reference. max_rounds follows the reference's create request: None
    /// resolves to the deployment default (DEFAULT_MAX_ROUNDS) rather than
    /// meaning "unbounded".
    pub fn new(objective: impl Into<String>, max_rounds: Option<u32>) -> Option<Self> {
        let mut goal = Self {
            objective: objective.into(),
            state: GoalState::Active,
            rounds: 0,
            max_rounds: max_rounds
                .filter(|cap| *cap >= 1)
                .unwrap_or(DEFAULT_MAX_ROUNDS),
            blocked_reason: None,
            activation: GoalActivation::Armed,
        };
        goal.sanitize();
        if goal.objective.is_empty() {
            return None;
        }
        Some(goal)
    }

    /// Trim and clamp the free text, and keep the fields consistent with the
    /// state: a goal that is not blocked does not carry a reason.
    pub fn sanitize(&mut self) {
        self.objective = self.objective.trim().to_string();
        if self.objective.chars().count() > MAX_OBJECTIVE {
            self.objective = self.objective.chars().take(MAX_OBJECTIVE).collect();
        }
        self.blocked_reason = self
            .blocked_reason
            .take()
            .and_then(|reason| BlockedReason::new(reason.code, reason.message));
        if self.max_rounds < 1 {
            self.max_rounds = DEFAULT_MAX_ROUNDS;
        }
        self.rounds = self.rounds.min(self.max_rounds);
        if !matches!(self.state, GoalState::Blocked) {
            self.blocked_reason = None;
        }
    }

    /// Whether another round may be counted right now: the goal is active,
    /// continuation is armed, and the cap has room.
    pub fn can_advance(&self) -> bool {
        self.state.is_open() && self.activation.is_armed() && !self.exhausted()
    }

    /// Whether the goal has spent every round it was given.
    pub fn exhausted(&self) -> bool {
        self.rounds >= self.max_rounds
    }

    /// Rounds left.
    pub fn remaining(&self) -> u32 {
        self.max_rounds.saturating_sub(self.rounds)
    }

    /// Count one round. False when the goal may not advance (not active, not
    /// armed, or out of rounds), in which case nothing changes.
    pub fn advance(&mut self) -> bool {
        if !self.can_advance() {
            return false;
        }
        self.rounds = self.rounds.saturating_add(1);
        true
    }

    /// Stop pursuing, keeping the option to resume. Only an active goal is
    /// paused; the reference rejects pause from every other phase.
    pub fn pause(&mut self) -> bool {
        if self.state != GoalState::Active {
            return false;
        }
        self.state = GoalState::Paused;
        self.activation = GoalActivation::Disarmed;
        true
    }

    /// Take up a stopped goal again, or re-arm an active goal after a session
    /// resume. False when the phase cannot resume, when an active goal is
    /// already armed, or when the round cap has no room left.
    pub fn resume(&mut self) -> bool {
        if matches!(self.state, GoalState::Completed) {
            return false;
        }
        if self.state == GoalState::Active && self.activation.is_armed() {
            return false;
        }
        if self.exhausted() {
            return false;
        }
        self.state = GoalState::Active;
        self.activation = GoalActivation::Armed;
        self.blocked_reason = None;
        true
    }

    /// Mark the objective achieved. Every phase but completed accepts it, and
    /// completion always disarms continuation.
    pub fn complete(&mut self) -> bool {
        if matches!(self.state, GoalState::Completed) {
            return false;
        }
        self.state = GoalState::Completed;
        self.activation = GoalActivation::Disarmed;
        self.blocked_reason = None;
        true
    }

    /// Stop on a concrete condition, recording its policy code and explanation.
    /// Only an active goal can block, and blocking always disarms continuation.
    pub fn block(&mut self, code: &str, message: impl Into<String>) -> bool {
        if self.state != GoalState::Active {
            return false;
        }
        let Some(reason) = BlockedReason::new(code, message) else {
            return false;
        };
        self.state = GoalState::Blocked;
        self.activation = GoalActivation::Disarmed;
        self.blocked_reason = Some(reason);
        true
    }

    /// Drop continuation authority without changing the durable phase, which is
    /// the reference's disarm().
    pub fn disarm(&mut self) {
        self.activation = GoalActivation::Disarmed;
    }

    /// Arm continuation without changing the durable phase. Only a goal that can
    /// actually continue may be armed.
    pub fn arm(&mut self) -> bool {
        if !self.state.is_open() || self.exhausted() {
            return false;
        }
        self.activation = GoalActivation::Armed;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_objective_is_not_a_goal() {
        assert!(Goal::new("   ", None).is_none());
        assert!(Goal::new("\n\t", Some(3)).is_none());
        let goal = Goal::new("  ship M3  ", None).expect("non-blank");
        assert_eq!(goal.objective, "ship M3");
        assert_eq!(goal.state, GoalState::Active);
        assert_eq!(goal.rounds, 0);
        assert_eq!(goal.max_rounds, DEFAULT_MAX_ROUNDS);
        assert_eq!(goal.activation, GoalActivation::Armed);
    }

    #[test]
    fn a_created_goal_is_capped_at_the_reference_default() {
        let goal = Goal::new("land it", None).expect("goal");
        assert_eq!(goal.max_rounds, DEFAULT_MAX_ROUNDS);
        assert_eq!(goal.remaining(), DEFAULT_MAX_ROUNDS);
        let named = Goal::new("land it", Some(3)).expect("goal");
        assert_eq!(named.max_rounds, 3);
    }

    #[test]
    fn rounds_advance_only_while_open_armed_and_capped() {
        let mut goal = Goal::new("land it", Some(2)).expect("goal");
        assert!(goal.advance());
        assert!(goal.advance());
        assert_eq!(goal.rounds, 2);
        assert!(goal.exhausted());
        assert!(!goal.can_advance());
        assert!(!goal.advance(), "a spent cap does not advance");
        assert_eq!(goal.rounds, 2);
        assert_eq!(goal.remaining(), 0);

        let mut open = Goal::new("capped", None).expect("goal");
        for _ in 0..5 {
            assert!(open.advance());
        }
        assert_eq!(open.rounds, 5);
        assert!(open.pause());
        assert!(!open.advance(), "a paused goal does not advance");
        assert!(open.resume());
        assert!(open.advance(), "a resumed goal advances again");
        open.disarm();
        assert!(!open.advance(), "a disarmed goal does not advance");
        assert!(open.arm());
        assert!(open.advance());
    }

    #[test]
    fn transitions_refuse_the_ones_that_make_no_sense() {
        let mut goal = Goal::new("objective", None).expect("goal");
        assert!(!goal.resume(), "an active armed goal is not resumed");
        assert!(goal.pause());
        assert!(!goal.pause(), "a paused goal is not paused twice");
        assert!(!goal.block("model-reported", "late"), "only active blocks");
        assert!(goal.resume());
        assert!(goal.complete());
        assert!(!goal.complete(), "a completed goal is not completed twice");
        assert!(!goal.pause(), "a completed goal is not paused");
        assert!(!goal.resume(), "a completed goal is not resumed");
        assert!(
            !goal.block("model-reported", "late"),
            "a completed goal is not blocked"
        );
    }

    #[test]
    fn pause_disarms_and_resume_rearms_from_every_stopped_phase() {
        let mut paused = Goal::new("objective", None).expect("goal");
        assert!(paused.pause());
        assert_eq!(paused.activation, GoalActivation::Disarmed);
        assert!(!paused.can_advance());
        assert!(paused.resume());
        assert_eq!(paused.state, GoalState::Active);
        assert_eq!(paused.activation, GoalActivation::Armed);

        let mut blocked = Goal::new("objective", None).expect("goal");
        assert!(blocked.block("round-limit", "rounds are gone"));
        assert_eq!(blocked.activation, GoalActivation::Disarmed);
        assert!(blocked.resume());
        assert_eq!(blocked.state, GoalState::Active);
        assert_eq!(blocked.blocked_reason, None, "resume clears the blocker");

        // An active goal that a session edge disarmed can be re-armed, but not
        // through resume when it is still armed.
        let mut edge = Goal::new("objective", None).expect("goal");
        edge.disarm();
        assert_eq!(edge.state, GoalState::Active);
        assert!(edge.resume());
        assert!(edge.activation.is_armed());
    }

    #[test]
    fn resume_refuses_an_exhausted_goal() {
        let mut goal = Goal::new("objective", Some(1)).expect("goal");
        assert!(goal.advance());
        assert!(goal.pause());
        assert!(!goal.resume(), "no rounds remain");
        assert_eq!(goal.state, GoalState::Paused);
        // Raising the cap is the way out, and then resume succeeds.
        goal.max_rounds = 2;
        assert!(goal.resume());
    }

    #[test]
    fn blocking_records_a_code_and_completing_clears_it() {
        let mut goal = Goal::new("objective", None).expect("goal");
        assert!(goal.block("round-limit", "  waiting on the signing key  "));
        assert_eq!(goal.state, GoalState::Blocked);
        assert_eq!(
            goal.blocked_reason,
            BlockedReason::new("round-limit", "waiting on the signing key")
        );
        assert!(
            !goal.block("round-limit", "   "),
            "a blank reason does not block"
        );
        assert!(!goal.block("Round Limit", "bad code"));
        assert_eq!(
            goal.blocked_reason
                .as_ref()
                .map(|reason| reason.code.as_str()),
            Some("round-limit"),
            "a refused block changes nothing"
        );
        assert!(goal.complete());
        assert_eq!(goal.blocked_reason, None);
    }

    #[test]
    fn a_blocker_code_is_lower_kebab_case() {
        assert!(BlockedReason::new("model-reported", "why").is_some());
        assert!(BlockedReason::new("round-limit", "why").is_some());
        assert!(BlockedReason::new("a1", "why").is_some());
        assert!(BlockedReason::new("", "why").is_none());
        assert!(BlockedReason::new("Upper", "why").is_none());
        assert!(BlockedReason::new("two--dashes", "why").is_none());
        assert!(BlockedReason::new("trailing-", "why").is_none());
        assert!(BlockedReason::new("under_score", "why").is_none());
        assert!(BlockedReason::new("ok", "  ").is_none());
    }

    #[test]
    fn sanitize_clamps_and_keeps_state_consistent() {
        let mut goal = Goal::new("x".repeat(MAX_OBJECTIVE + 50), None).expect("goal");
        assert_eq!(goal.objective.chars().count(), MAX_OBJECTIVE);

        goal.blocked_reason = BlockedReason::new(MODEL_REPORTED, "y".repeat(MAX_REASON + 10));
        goal.state = GoalState::Blocked;
        goal.sanitize();
        assert_eq!(
            goal.blocked_reason
                .as_ref()
                .map(|reason| reason.message.chars().count()),
            Some(MAX_REASON)
        );

        goal.state = GoalState::Active;
        goal.sanitize();
        assert_eq!(
            goal.blocked_reason, None,
            "a non-blocked goal carries no reason"
        );

        let mut capped = Goal::new("capped", Some(2)).expect("goal");
        capped.rounds = 9;
        capped.sanitize();
        assert_eq!(capped.rounds, 2, "rounds past the cap are clamped");
    }

    #[test]
    fn activation_is_never_persisted_and_a_reload_is_disarmed() {
        let mut goal = Goal::new("objective", Some(4)).expect("goal");
        goal.advance();
        assert!(goal.activation.is_armed());
        let json = serde_json::to_string(&goal).expect("serialize");
        assert!(
            !json.contains("activation") && !json.contains("armed"),
            "activation is process-local, not durable: {json}"
        );
        let back: Goal = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.activation, GoalActivation::Disarmed);
        assert_eq!(back.rounds, goal.rounds);
        assert_eq!(back.max_rounds, goal.max_rounds);
    }

    #[test]
    fn older_files_load_with_the_default_cap_and_the_legacy_blocker() {
        let legacy: Goal = serde_json::from_str(r#"{"objective":"old"}"#).expect("legacy");
        assert_eq!(legacy.state, GoalState::Active);
        assert_eq!(legacy.rounds, 0);
        assert_eq!(legacy.max_rounds, DEFAULT_MAX_ROUNDS);
        assert_eq!(legacy.activation, GoalActivation::Disarmed);

        let mut legacy_blocked: Goal = serde_json::from_str(
            r#"{"objective":"old","state":"blocked","blocked_reason":"no key"}"#,
        )
        .expect("legacy");
        assert_eq!(
            legacy_blocked.blocked_reason,
            BlockedReason::new(MODEL_REPORTED, "no key")
        );
        legacy_blocked.sanitize();
        assert_eq!(
            legacy_blocked.blocked_reason,
            BlockedReason::new(MODEL_REPORTED, "no key")
        );

        let no_cap: Goal =
            serde_json::from_str(r#"{"objective":"old","rounds":4}"#).expect("legacy");
        assert_eq!(no_cap.max_rounds, DEFAULT_MAX_ROUNDS);
        let null_cap: Goal =
            serde_json::from_str(r#"{"objective":"old","max_rounds":null}"#).expect("legacy");
        assert_eq!(null_cap.max_rounds, DEFAULT_MAX_ROUNDS);
    }
}
