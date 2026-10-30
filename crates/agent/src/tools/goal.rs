//! The get_goal / update_goal tools and the shared store a host settles onto
//! its durable session goal.
//!
//! The durable goal domain lives in [crate::goal]; this module is the model's
//! view of it. A host seeds a [GoalStore] with the session's goal before a
//! turn, hands the tools clones of it, and applies [GoalStore::take_settle]
//! when the turn ends. Until that settle the store is a snapshot plus one
//! recorded request - never a second source of truth for the durable phase.

use std::sync::{Arc, Mutex};

use react::tool::{Tool, ToolError};
use serde_json::{json, Value};

use crate::goal::{Goal, MODEL_REPORTED};

/// A settle transition the model asked for during a turn.
///
/// The host applies it to the durable goal when the turn ends, which is what
/// lets a completion stop the automatic round loop instead of leaving an
/// active goal that can only reach the round cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalSettle {
    /// The whole objective is achieved.
    Complete,
    /// A concrete condition stops progress; the code and message ride along.
    Blocked {
        /// The stable lower-kebab-case policy code.
        code: String,
        /// The explanation the host records on the durable blocker.
        message: String,
    },
}

/// Shared, cheaply cloneable handle to the goal of one running turn.
///
/// The host seeds it from the session's durable goal and keeps one clone for
/// the tools the agent carries; when the turn ends it takes the recorded
/// transition and settles it onto [crate::goal::Goal]. A lock poisoned by a
/// panicking writer leaves the store untouched, so a failed call never panics
/// the turn.
#[derive(Debug, Clone)]
pub struct GoalStore {
    inner: Arc<Mutex<GoalStoreInner>>,
}

/// The locked half of a [GoalStore].
#[derive(Debug)]
struct GoalStoreInner {
    /// The goal as the model sees it, transitions applied.
    goal: Goal,
    /// The transition the host still has to settle onto the durable goal.
    settle: Option<GoalSettle>,
}

impl GoalStore {
    /// Seed a store with the session's current goal.
    pub fn new(goal: Goal) -> Self {
        Self {
            inner: Arc::new(Mutex::new(GoalStoreInner { goal, settle: None })),
        }
    }

    /// The goal as the model sees it now.
    pub fn snapshot(&self) -> Option<Goal> {
        self.inner.lock().ok().map(|guard| guard.goal.clone())
    }

    /// Mark the objective achieved. False when it already is, or when the
    /// store cannot be locked.
    pub fn complete(&self) -> bool {
        let Ok(mut guard) = self.inner.lock() else {
            return false;
        };
        if !guard.goal.complete() {
            return false;
        }
        guard.settle = Some(GoalSettle::Complete);
        true
    }

    /// Record a blocker. False when the goal cannot block (only an active goal
    /// can), when the reason is invalid (a blank message or a code that is not
    /// lower kebab case), or when the store cannot be locked.
    pub fn block(&self, code: &str, message: &str) -> bool {
        let Ok(mut guard) = self.inner.lock() else {
            return false;
        };
        if !guard.goal.block(code, message) {
            return false;
        }
        let Some(reason) = guard.goal.blocked_reason.clone() else {
            return false;
        };
        guard.settle = Some(GoalSettle::Blocked {
            code: reason.code,
            message: reason.message,
        });
        true
    }

    /// Take the transition the model recorded, if any.
    ///
    /// The host calls this once when the turn ends; taking it keeps one turn
    /// from settling twice (a second take reads None).
    pub fn take_settle(&self) -> Option<GoalSettle> {
        self.inner
            .lock()
            .ok()
            .and_then(|mut guard| guard.settle.take())
    }

    /// Whether the model has recorded any settle transition that is still
    /// waiting for the host.
    pub fn has_settle(&self) -> bool {
        self.inner
            .lock()
            .map(|guard| guard.settle.is_some())
            .unwrap_or(false)
    }
}

/// The wire view of a goal the two tools return.
fn goal_view(goal: &Goal) -> Value {
    json!({
        "objective": goal.objective,
        "state": goal.state,
        "rounds": goal.rounds,
        "max_rounds": goal.max_rounds,
        "blocked_reason": goal.blocked_reason.as_ref().map(|reason| json!({
            "code": reason.code,
            "message": reason.message,
        })),
        "activation": goal.activation.as_str(),
    })
}

/// The get_goal tool: the model reads the durable goal it is pursuing.
pub struct GetGoalTool {
    /// The turn's goal handle.
    store: GoalStore,
}

impl GetGoalTool {
    /// Attach the tool to the store the host seeded for this turn.
    pub fn new(store: GoalStore) -> Self {
        Self { store }
    }

    /// The shared store backing this tool.
    pub fn store(&self) -> &GoalStore {
        &self.store
    }
}

impl Tool for GetGoalTool {
    fn name(&self) -> &str {
        "get_goal"
    }

    fn description(&self) -> String {
        "Read the current session goal: its objective, durable phase \
         (active, paused, completed, or blocked), rounds used against the cap, \
         and any blocker. Check it before claiming the objective is achieved, \
         so the decision rests on the durable goal rather than on memory."
            .to_string()
    }

    fn category(&self) -> String {
        "planning".to_string()
    }

    fn json_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }

    fn run(&self, _input: &Value) -> Result<Value, ToolError> {
        let Some(goal) = self.store.snapshot() else {
            return Err(ToolError::Failed(
                "the goal store is unavailable".to_string(),
            ));
        };
        Ok(json!({ "goal": goal_view(&goal) }))
    }
}

/// Wire shape accepted by [UpdateGoalTool].
#[derive(Debug, serde::Deserialize)]
struct UpdateGoalInput {
    /// Either "complete" or "blocked".
    action: String,
    /// The blocker explanation, required with action "blocked".
    blocked_reason: Option<String>,
}

/// The update_goal tool: the model settles the goal it is pursuing.
///
/// Only the two settlings an automatic continuation may make are offered -
/// complete and blocked. Edit, pause and resume are operator decisions that the
/// /goal command and the goal bar own, exactly as the reference documents for
/// its own tool package.
pub struct UpdateGoalTool {
    /// The turn's goal handle.
    store: GoalStore,
}

impl UpdateGoalTool {
    /// Attach the tool to the store the host seeded for this turn.
    pub fn new(store: GoalStore) -> Self {
        Self { store }
    }

    /// The shared store backing this tool.
    pub fn store(&self) -> &GoalStore {
        &self.store
    }
}

impl Tool for UpdateGoalTool {
    fn name(&self) -> &str {
        "update_goal"
    }

    fn description(&self) -> String {
        "Update the current session goal. Action complete records that the \
         whole objective is achieved and stops further automatic rounds; call \
         it only after gathering evidence for the whole objective. Action \
         blocked records the concrete condition that stops progress and \
         disarms continuation. These are the only updates an automatic goal \
         round may make."
            .to_string()
    }

    fn category(&self) -> String {
        "planning".to_string()
    }

    fn json_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["complete", "blocked"],
                    "description": "What to record on the goal."
                },
                "blocked_reason": {
                    "type": "string",
                    "description": "Required with action blocked: one sentence naming the concrete condition that stops progress."
                }
            },
            "required": ["action"],
            "additionalProperties": false
        })
    }

    fn run(&self, input: &Value) -> Result<Value, ToolError> {
        let parsed: UpdateGoalInput = serde_json::from_value(input.clone())
            .map_err(|e| ToolError::Failed(format!("invalid update_goal input: {e}")))?;
        let changed = match parsed.action.as_str() {
            "complete" => self.store.complete(),
            "blocked" => {
                let Some(reason) = parsed
                    .blocked_reason
                    .as_deref()
                    .map(str::trim)
                    .filter(|reason| !reason.is_empty())
                else {
                    return Err(ToolError::Failed(
                        "the action blocked requires blocked_reason: name the concrete condition that stops progress".to_string(),
                    ));
                };
                self.store.block(MODEL_REPORTED, reason)
            }
            other => {
                return Err(ToolError::Failed(format!(
                    "unknown update_goal action {other}: use complete or blocked"
                )));
            }
        };
        if !changed {
            return Err(ToolError::Failed(
                "the goal could not take that transition: only an active goal can block, and a completed goal stays completed".to_string(),
            ));
        }
        let Some(goal) = self.store.snapshot() else {
            return Err(ToolError::Failed(
                "the goal store is unavailable".to_string(),
            ));
        };
        Ok(json!({ "goal": goal_view(&goal) }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal::GoalState;

    fn seeded() -> GoalStore {
        let goal = Goal::new("ship the palette", Some(8)).expect("a goal");
        GoalStore::new(goal)
    }

    #[test]
    fn get_goal_reports_the_durable_fields() {
        let tool = GetGoalTool::new(seeded());
        let out = tool.run(&json!({})).expect("a read");
        let goal = &out["goal"];
        assert_eq!(goal["objective"], "ship the palette");
        assert_eq!(goal["state"], "active");
        assert_eq!(goal["rounds"], 0);
        assert_eq!(goal["max_rounds"], 8);
        assert_eq!(goal["activation"], "armed");
        assert!(goal["blocked_reason"].is_null());
    }

    #[test]
    fn complete_records_one_settle_and_flips_the_view() {
        let store = seeded();
        let tool = UpdateGoalTool::new(store.clone());
        let out = tool
            .run(&json!({"action": "complete"}))
            .expect("the completion");
        assert_eq!(out["goal"]["state"], "completed");
        assert_eq!(store.take_settle(), Some(GoalSettle::Complete));
        assert!(
            store.take_settle().is_none(),
            "the host settles one transition once"
        );
        // The durable transition rules still apply: a completed goal does not
        // complete twice.
        assert!(tool.run(&json!({"action": "complete"})).is_err());
    }

    #[test]
    fn blocked_records_the_model_reported_code_and_message() {
        let store = seeded();
        let tool = UpdateGoalTool::new(store.clone());
        tool.run(&json!({
            "action": "blocked",
            "blocked_reason": "  the signing key is not in the vault  "
        }))
        .expect("the blocker");
        assert_eq!(
            store.take_settle(),
            Some(GoalSettle::Blocked {
                code: MODEL_REPORTED.to_string(),
                message: "the signing key is not in the vault".to_string(),
            })
        );
        let goal = store.snapshot().expect("a snapshot");
        assert_eq!(goal.state, GoalState::Blocked);
        assert_eq!(
            goal.blocked_reason.as_ref().map(|r| r.code.as_str()),
            Some(MODEL_REPORTED)
        );
    }

    #[test]
    fn blocked_without_a_reason_is_rejected_and_sets_nothing() {
        let store = seeded();
        let tool = UpdateGoalTool::new(store.clone());
        assert!(tool
            .run(&json!({"action": "blocked", "blocked_reason": "   "}))
            .is_err());
        assert!(tool.run(&json!({"action": "blocked"})).is_err());
        assert!(store.take_settle().is_none());
        assert_eq!(
            store.snapshot().expect("a snapshot").state,
            GoalState::Active
        );
    }

    #[test]
    fn only_the_two_settling_actions_are_accepted() {
        let tool = UpdateGoalTool::new(seeded());
        let error = tool
            .run(&json!({"action": "pause"}))
            .expect_err("an operator verb is not a model verb");
        assert!(error.to_string().contains("unknown update_goal action"));
        assert!(tool.run(&json!({})).is_err(), "action is required");
    }

    #[test]
    fn schema_declares_the_action_contract() {
        let schema = UpdateGoalTool::new(seeded()).json_schema();
        assert_eq!(schema["required"][0], "action");
        let actions = &schema["properties"]["action"]["enum"];
        assert_eq!(actions[0], "complete");
        assert_eq!(actions[1], "blocked");
    }
}
