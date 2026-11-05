//! Durable working goals: the objective a session pursues and the store the
//! model's `get_goal`/`update_goal` tools write to.

use napi::bindgen_prelude::*;
use napi_derive::napi;

use agent::goal::{Goal as CoreGoal, GoalState, MAX_OBJECTIVE, MAX_REASON};

fn state_name(state: GoalState) -> &'static str {
  match state {
    GoalState::Active => "active",
    GoalState::Paused => "paused",
    GoalState::Completed => "completed",
    GoalState::Blocked => "blocked",
  }
}

fn goal_json(goal: &CoreGoal) -> serde_json::Value {
  serde_json::json!({
      "objective": goal.objective,
      "state": state_name(goal.state),
      "rounds": goal.rounds,
      "max_rounds": goal.max_rounds,
      "blocked_reason": goal.blocked_reason.as_ref().map(|reason| serde_json::json!({
          "code": reason.code,
          "message": reason.message,
      })),
      "activation": goal.activation.as_str(),
  })
}

/// Why a blocked goal stopped: a stable policy code plus an explanation.
#[napi(object)]
pub struct GoalBlockedReason {
  /// The policy-owned lower-kebab-case code.
  pub code: String,
  /// The human-readable explanation.
  pub message: String,
}

/// A durable working goal.
///
/// Every field is sanitized on construction, so a host can pass whatever a
/// person typed and still get a value every other host can render.
#[napi]
pub struct Goal {
  inner: CoreGoal,
}

#[napi]
impl Goal {
  #[napi(factory)]
  /// Create an active, armed goal. Throws when the objective is blank.
  pub fn create(objective: String, max_rounds: Option<u32>) -> Result<Self> {
    let goal = CoreGoal::new(objective, max_rounds).ok_or_else(|| {
      Error::new(
        napi::Status::InvalidArg,
        "a goal needs a non-blank objective",
      )
    })?;
    Ok(Self { inner: goal })
  }

  #[napi(factory, js_name = "fromJSON")]
  /// Restore a goal from its JSON snapshot.
  pub fn from_json(json: String) -> Result<Self> {
    let mut goal: CoreGoal = serde_json::from_str(&json).map_err(|error| {
      Error::new(
        napi::Status::InvalidArg,
        format!("invalid goal JSON: {error}"),
      )
    })?;
    goal.sanitize();
    Ok(Self { inner: goal })
  }

  #[napi(js_name = "toJSON")]
  /// The goal as a plain JSON object.
  pub fn to_json(&self) -> serde_json::Value {
    goal_json(&self.inner)
  }

  #[napi(getter)]
  /// What the work is for, as one statement.
  pub fn objective(&self) -> String {
    self.inner.objective.clone()
  }

  #[napi(getter)]
  /// One of `active`, `paused`, `completed` or `blocked`.
  pub fn state(&self) -> String {
    state_name(self.inner.state).to_string()
  }

  #[napi(getter)]
  /// Rounds spent pursuing the goal.
  pub fn rounds(&self) -> u32 {
    self.inner.rounds
  }

  #[napi(getter)]
  /// The round cap.
  pub fn max_rounds(&self) -> u32 {
    self.inner.max_rounds
  }

  #[napi(getter)]
  /// `armed` or `disarmed`.
  pub fn activation(&self) -> String {
    self.inner.activation.as_str().to_string()
  }

  #[napi(getter)]
  /// The blocker, only while the goal is blocked.
  pub fn blocked_reason(&self) -> Option<GoalBlockedReason> {
    self
      .inner
      .blocked_reason
      .as_ref()
      .map(|reason| GoalBlockedReason {
        code: reason.code.clone(),
        message: reason.message.clone(),
      })
  }

  #[napi(getter)]
  /// Rounds left before the cap.
  pub fn remaining(&self) -> u32 {
    self.inner.remaining()
  }

  #[napi(getter)]
  /// Whether another round may be counted right now.
  pub fn can_advance(&self) -> bool {
    self.inner.can_advance()
  }

  #[napi(getter)]
  /// Whether every round has been spent.
  pub fn exhausted(&self) -> bool {
    self.inner.exhausted()
  }

  #[napi(getter)]
  /// The largest objective this binding accepts, in characters.
  pub fn max_objective(&self) -> u32 {
    MAX_OBJECTIVE as u32
  }

  #[napi(getter)]
  /// The largest blocked explanation this binding accepts, in characters.
  pub fn max_reason(&self) -> u32 {
    MAX_REASON as u32
  }

  #[napi]
  /// Count one round; false when the goal may not advance.
  pub fn advance(&mut self) -> bool {
    self.inner.advance()
  }

  #[napi]
  /// Stop pursuing, keeping the option to resume.
  pub fn pause(&mut self) -> bool {
    self.inner.pause()
  }

  #[napi]
  /// Take up a stopped goal again.
  pub fn resume(&mut self) -> bool {
    self.inner.resume()
  }

  #[napi]
  /// Mark the objective achieved.
  pub fn complete(&mut self) -> bool {
    self.inner.complete()
  }

  #[napi]
  /// Stop on a concrete condition; the code must be lower-kebab-case.
  pub fn block(&mut self, code: String, message: String) -> bool {
    self.inner.block(&code, message)
  }

  #[napi]
  /// Allow automatic continuation without changing the phase.
  pub fn arm(&mut self) -> bool {
    self.inner.arm()
  }

  #[napi]
  /// Drop continuation authority without changing the phase.
  pub fn disarm(&mut self) {
    self.inner.disarm();
  }

  #[napi]
  /// Re-apply the field bounds and consistency rules in place.
  pub fn sanitize(&mut self) {
    self.inner.sanitize();
  }
}

/// The goal handle one running turn's tools write to.
///
/// A host seeds it with the session's current goal, hands it to
/// `Agent.addGoalTools`, and applies `takeSettle` when the turn ends.
#[napi]
pub struct GoalStore {
  pub(crate) inner: agent::tools::GoalStore,
}

#[napi]
impl GoalStore {
  #[napi(constructor)]
  /// Seed a store from `goal`.
  pub fn new(goal: &Goal) -> Self {
    Self {
      inner: agent::tools::GoalStore::new(goal.inner.clone()),
    }
  }

  #[napi]
  /// The goal as the model sees it now.
  pub fn snapshot(&self) -> Option<Goal> {
    self.inner.snapshot().map(|inner| Goal { inner })
  }

  #[napi]
  /// Mark the objective achieved.
  pub fn complete(&self) -> bool {
    self.inner.complete()
  }

  #[napi]
  /// Record a blocker; the code must be lower-kebab-case.
  pub fn block(&self, code: String, message: String) -> bool {
    self.inner.block(&code, &message)
  }

  #[napi]
  /// Take the transition the model recorded, if any.
  pub fn take_settle(&self) -> Option<serde_json::Value> {
    self.inner.take_settle().map(|settle| match settle {
      agent::tools::GoalSettle::Complete => serde_json::json!({ "type": "complete" }),
      agent::tools::GoalSettle::Blocked { code, message } => serde_json::json!({
          "type": "blocked",
          "code": code,
          "message": message,
      }),
    })
  }

  #[napi]
  /// Whether a settle transition is still waiting for the host.
  pub fn has_settle(&self) -> bool {
    self.inner.has_settle()
  }
}
