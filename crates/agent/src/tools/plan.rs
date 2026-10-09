//! The `update_plan` tool and its shared store: a working plan the model
//! maintains across a multi-step task and hosts (chat UIs, bindings) render.

use std::sync::{Arc, Mutex};

use react::tool::{Tool, ToolError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Cap on plan items so a runaway loop cannot grow the store without bound.
const MAX_ITEMS: usize = 100;
/// Cap on a single item's text, in characters.
const MAX_TEXT: usize = 500;

/// Progress state of one plan item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    /// Not started yet.
    #[default]
    Pending,
    /// The step the agent is working on right now.
    InProgress,
    /// Finished.
    Completed,
}

/// One tracked step of the agent's working plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanItem {
    /// Short, human-readable step description.
    pub text: String,
    /// Progress state of the step.
    #[serde(default)]
    pub status: PlanStatus,
}

/// Shared, cheaply cloneable store for the working plan.
///
/// The agent owns one instance, [`PlanTool`] and the host share clones of
/// it, so a tool call is immediately visible through [`Agent::plan_items`].
///
/// [`Agent::plan_items`]: crate::Agent::plan_items
#[derive(Debug, Clone, Default)]
pub struct PlanStore {
    items: Arc<Mutex<Vec<PlanItem>>>,
}

impl PlanStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of the current plan items.
    pub fn items(&self) -> Vec<PlanItem> {
        self.items
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Replace the whole plan.
    ///
    /// Blank items are dropped, item text is trimmed and truncated, and the
    /// list is capped at 100 entries. A lock that cannot be taken
    /// (poisoned by a panicking writer) leaves the store untouched, so a
    /// failed write never panics the caller.
    pub fn replace(&self, items: Vec<PlanItem>) {
        let cleaned: Vec<PlanItem> = items
            .into_iter()
            .filter_map(|mut item| {
                let text = item.text.trim();
                if text.is_empty() {
                    return None;
                }
                item.text = text.chars().take(MAX_TEXT).collect();
                Some(item)
            })
            .take(MAX_ITEMS)
            .collect();
        if let Ok(mut guard) = self.items.lock() {
            *guard = cleaned;
        }
    }

    /// Forget the whole plan (an empty [`PlanTool`] call does the same).
    pub fn clear(&self) {
        if let Ok(mut guard) = self.items.lock() {
            guard.clear();
        }
    }
}

/// The `update_plan` tool: the model replaces its entire working plan in a
/// single call, and hosts read the same [`PlanStore`] to render the plan.
pub struct PlanTool {
    store: PlanStore,
}

impl PlanTool {
    /// Attach a tool to `store` — usually the store returned by
    /// [`Agent::plan`].
    ///
    /// [`Agent::plan`]: crate::Agent::plan
    pub fn new(store: PlanStore) -> Self {
        Self { store }
    }

    /// The shared store backing this tool.
    pub fn store(&self) -> &PlanStore {
        &self.store
    }
}

/// Wire shape accepted by [`PlanTool`]: the full replacement list.
#[derive(Debug, Deserialize)]
struct UpdatePlanInput {
    /// Every step of the plan, current statuses included.
    items: Vec<PlanItem>,
}

impl Tool for PlanTool {
    fn name(&self) -> &str {
        "update_plan"
    }

    fn description(&self) -> String {
        "Track a multi-step task by replacing the whole working plan. \
         Pass `items` as the COMPLETE list of steps — each `{text, status}` \
         with status `pending`, `in_progress`, or `completed` — because every \
         call overwrites the previous plan. Keep at most a handful of steps, \
         mark exactly one step `in_progress`, and pass an empty list when the \
         plan no longer applies. The plan is shown to the user and persisted."
            .to_string()
    }

    fn category(&self) -> String {
        "planning".to_string()
    }

    fn json_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "description": "The complete plan: every step with its status.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "text": {
                                "type": "string",
                                "description": "One concise step description."
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"],
                                "description": "Progress of this step."
                            }
                        },
                        "required": ["text", "status"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["items"],
            "additionalProperties": false
        })
    }

    fn run(&self, input: &Value) -> Result<Value, ToolError> {
        let parsed: UpdatePlanInput = serde_json::from_value(input.clone())
            .map_err(|e| ToolError::Failed(format!("invalid update_plan input: {e}")))?;
        self.store.replace(parsed.items);
        let items = self.store.items();
        let pending = items
            .iter()
            .filter(|i| i.status == PlanStatus::Pending)
            .count();
        let in_progress = items
            .iter()
            .filter(|i| i.status == PlanStatus::InProgress)
            .count();
        let completed = items
            .iter()
            .filter(|i| i.status == PlanStatus::Completed)
            .count();
        serde_json::to_value(serde_json::json!({
            "total": items.len(),
            "pending": pending,
            "in_progress": in_progress,
            "completed": completed,
            "items": items,
        }))
        .map_err(|e| ToolError::Failed(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> PlanTool {
        PlanTool::new(PlanStore::new())
    }

    #[test]
    fn replaces_the_whole_plan() {
        let tool = tool();
        let first = serde_json::json!({
            "items": [
                {"text": "design", "status": "completed"},
                {"text": "build", "status": "in_progress"},
                {"text": "verify", "status": "pending"}
            ]
        });
        let out = tool.run(&first).expect("first run");
        assert_eq!(out["total"], 3);
        assert_eq!(out["completed"], 1);
        assert_eq!(out["in_progress"], 1);

        // The next call is a full replacement, not an append.
        let second = serde_json::json!({"items": [{"text": "verify", "status": "in_progress"}]});
        tool.run(&second).expect("second run");
        let items = tool.store().items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "verify");
        assert_eq!(items[0].status, PlanStatus::InProgress);
    }

    #[test]
    fn empty_list_clears_the_plan() {
        let tool = tool();
        tool.run(&serde_json::json!({"items": [{"text": "a"}]}))
            .expect("seed");
        tool.run(&serde_json::json!({"items": []})).expect("clear");
        assert!(tool.store().items().is_empty());
    }

    #[test]
    fn missing_items_or_unknown_status_is_rejected() {
        let tool = tool();
        assert!(tool.run(&serde_json::json!({})).is_err());
        assert!(tool
            .run(&serde_json::json!({"items": [{"text": "a", "status": "done"}]}))
            .is_err());
        // The store is untouched by a rejected call.
        assert!(tool.store().items().is_empty());
    }

    #[test]
    fn blank_items_are_dropped_and_the_list_is_capped() {
        let tool = tool();
        let mut items: Vec<Value> = (0..MAX_ITEMS + 50)
            .map(|i| serde_json::json!({"text": format!("step {i}"), "status": "pending"}))
            .collect();
        items.push(serde_json::json!({"text": "   ", "status": "pending"}));
        tool.run(&serde_json::json!({"items": items})).expect("run");
        let store_items = tool.store().items();
        assert_eq!(store_items.len(), MAX_ITEMS);
        assert!(store_items.iter().all(|i| !i.text.trim().is_empty()));
    }

    #[test]
    fn item_text_is_trimmed_and_truncated() {
        let tool = tool();
        let long = "x".repeat(MAX_TEXT + 40);
        tool.run(&serde_json::json!({"items": [{"text": format!("  {long}  ")}]}))
            .expect("run");
        let items = tool.store().items();
        assert_eq!(items[0].text.len(), MAX_TEXT);
    }

    #[test]
    fn schema_declares_the_items_contract() {
        let schema = tool().json_schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"][0], "items");
        let status = &schema["properties"]["items"]["items"]["properties"]["status"];
        assert_eq!(status["enum"][1], "in_progress");
    }
}
