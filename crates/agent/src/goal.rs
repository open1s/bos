//! A durable working goal: the objective a session is pursuing, how many rounds
//! it has taken, and why it stopped.
//!
//! The plan ([`crate::tools::PlanStore`]) answers "what steps are left in this
//! task"; a goal answers the larger question "what is all of this for", and it
//! outlives any single turn. Hosts render it and drive its transitions, but the
//! model lives here so every host agrees on what `blocked` means and no UI has
//! to re-implement a state machine.

use serde::{Deserialize, Serialize};

/// Cap on an objective's length, in characters.
pub const MAX_OBJECTIVE: usize = 2000;
/// Cap on a blocked reason's length, in characters.
pub const MAX_REASON: usize = 500;

/// Where a goal stands.
///
/// The distinction that matters is between a goal that is *stopped* and one that
/// is *settled*: a paused goal can be resumed, while a completed or blocked one
/// has reached a state only a new decision can move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalState {
    /// Being pursued; rounds may advance.
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

/// The goal a session is pursuing.
///
/// Every field is sanitized on the way in (see [`Goal::sanitize`]), so a host
/// can hand the store whatever a person typed and the store still holds a value
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
    /// Optional cap on rounds; `None` means unbounded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rounds: Option<u32>,
    /// Why a blocked goal stopped, kept only while it is blocked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
}

impl Goal {
    /// Start a fresh, active goal. `None` when the objective is blank once
    /// trimmed — an empty goal is the absence of a goal, not a goal with no
    /// objective.
    pub fn new(objective: impl Into<String>, max_rounds: Option<u32>) -> Option<Self> {
        let mut goal = Self {
            objective: objective.into(),
            state: GoalState::Active,
            rounds: 0,
            max_rounds,
            blocked_reason: None,
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
            .map(|reason| reason.trim().to_string())
            .filter(|reason| !reason.is_empty())
            .map(|reason| {
                if reason.chars().count() > MAX_REASON {
                    reason.chars().take(MAX_REASON).collect()
                } else {
                    reason
                }
            });
        if !matches!(self.state, GoalState::Blocked) {
            self.blocked_reason = None;
        }
        if let Some(max) = self.max_rounds {
            self.rounds = self.rounds.min(max);
        }
    }

    /// Whether another round may be counted right now: the goal is active and
    /// has not exhausted its cap.
    pub fn can_advance(&self) -> bool {
        self.state.is_open() && !self.exhausted()
    }

    /// Whether a capped goal has spent every round it was given.
    pub fn exhausted(&self) -> bool {
        self.max_rounds.is_some_and(|max| self.rounds >= max)
    }

    /// Rounds left, when the goal is capped.
    pub fn remaining(&self) -> Option<u32> {
        self.max_rounds.map(|max| max.saturating_sub(self.rounds))
    }

    /// Count one round. `false` when the goal may not advance (not active, or
    /// out of rounds), in which case nothing changes.
    pub fn advance(&mut self) -> bool {
        if !self.can_advance() {
            return false;
        }
        self.rounds = self.rounds.saturating_add(1);
        true
    }

    /// Stop pursuing, keeping the option to resume.
    pub fn pause(&mut self) -> bool {
        if !self.state.is_open() {
            return false;
        }
        self.state = GoalState::Paused;
        true
    }

    /// Take up a paused goal again.
    pub fn resume(&mut self) -> bool {
        if self.state != GoalState::Paused {
            return false;
        }
        self.state = GoalState::Active;
        true
    }

    /// Mark the objective achieved.
    pub fn complete(&mut self) -> bool {
        if self.state.is_settled() {
            return false;
        }
        self.state = GoalState::Completed;
        self.blocked_reason = None;
        true
    }

    /// Stop on a concrete condition, recording why. A goal cannot be blocked
    /// without a reason: "blocked" that cannot say what blocks it is not a
    /// fact anyone can act on.
    pub fn block(&mut self, reason: impl Into<String>) -> bool {
        if self.state.is_settled() {
            return false;
        }
        self.state = GoalState::Blocked;
        self.blocked_reason = Some(reason.into());
        self.sanitize();
        self.blocked_reason.is_some()
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
    }

    #[test]
    fn rounds_advance_only_while_open_and_capped() {
        let mut goal = Goal::new("land it", Some(2)).expect("goal");
        assert!(goal.advance());
        assert!(goal.advance());
        assert_eq!(goal.rounds, 2);
        assert!(goal.exhausted());
        assert!(!goal.can_advance());
        assert!(!goal.advance(), "a spent cap does not advance");
        assert_eq!(goal.rounds, 2);
        assert_eq!(goal.remaining(), Some(0));

        let mut open = Goal::new("uncapped", None).expect("goal");
        assert_eq!(open.remaining(), None);
        for _ in 0..5 {
            assert!(open.advance());
        }
        assert_eq!(open.rounds, 5);
        assert!(open.pause());
        assert!(!open.advance(), "a paused goal does not advance");
    }

    #[test]
    fn transitions_refuse_the_ones_that_make_no_sense() {
        let mut goal = Goal::new("objective", None).expect("goal");
        assert!(!goal.resume(), "an active goal is not resumed");
        assert!(goal.pause());
        assert!(!goal.pause(), "a paused goal is not paused twice");
        assert!(goal.resume());
        assert!(goal.complete());
        assert!(!goal.complete(), "a settled goal is not completed twice");
        assert!(!goal.pause(), "a settled goal is not paused");
        assert!(!goal.block("late"), "a settled goal is not blocked");
    }

    #[test]
    fn blocking_records_a_reason_and_completing_clears_it() {
        let mut goal = Goal::new("objective", None).expect("goal");
        assert!(goal.block("  waiting on the signing key  "));
        assert_eq!(goal.state, GoalState::Blocked);
        assert_eq!(
            goal.blocked_reason.as_deref(),
            Some("waiting on the signing key")
        );
        assert!(!goal.block("   "), "a blank reason does not block");
        assert!(goal.state.is_settled());
    }

    #[test]
    fn sanitize_clamps_and_keeps_state_consistent() {
        let mut goal = Goal::new("x".repeat(MAX_OBJECTIVE + 50), None).expect("goal");
        assert_eq!(goal.objective.chars().count(), MAX_OBJECTIVE);

        goal.blocked_reason = Some("y".repeat(MAX_REASON + 10));
        goal.state = GoalState::Blocked;
        goal.sanitize();
        assert_eq!(
            goal.blocked_reason.as_ref().map(|r| r.chars().count()),
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
    fn a_stored_goal_round_trips_and_older_files_load() {
        let mut goal = Goal::new("objective", Some(4)).expect("goal");
        goal.advance();
        goal.block("no key");
        let json = serde_json::to_string(&goal).expect("serialize");
        let back: Goal = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, goal);

        // A file written before goals carried these fields still loads.
        let legacy: Goal = serde_json::from_str(r#"{"objective":"old"}"#).expect("legacy");
        assert_eq!(legacy.state, GoalState::Active);
        assert_eq!(legacy.rounds, 0);
        assert_eq!(legacy.max_rounds, None);
        assert_eq!(legacy.blocked_reason, None);
    }
}
