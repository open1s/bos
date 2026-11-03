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
        "Record and update a task list to plan multi-step work and show progress; \
         skip it for trivial single-step tasks. Add one item per concrete step \
         before you start. Pass `items` as the COMPLETE list of steps — each \
         `{text, status}` with status `pending`, `in_progress`, or `completed` — \
         every call overwrites the previous plan. While work remains, keep at \
         most one item `in_progress`. Mark each item `completed` as soon as it \
         is done, and pass an empty list when the plan no longer applies."
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
                    "description": "The complete replacement plan. Use an empty array to clear it; include at most 100 steps.",
                    "maxItems": 100,
                    "items": {
                        "type": "object",
                        "properties": {
                            "text": {
                                "type": "string",
                                "minLength": 1,
                                "maxLength": 500,
                                "description": "One concise, non-empty step description of at most 500 characters."
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

        // The cap applies to the steps that will be STORED: blank entries are
        // dropped by PlanStore::replace, so counting them here refused a plan
        // of 100 real steps plus one blank line.
        let stored = parsed
            .items
            .iter()
            .filter(|item| !item.text.trim().is_empty())
            .count();
        if stored > MAX_ITEMS {
            return Err(ToolError::InvalidInput(format!(
                "items must contain at most {MAX_ITEMS} steps"
            )));
        }
        for item in &parsed.items {
            if item.text.trim().is_empty() {
                continue;
            }
            if item.text.trim().chars().count() > MAX_TEXT {
                return Err(ToolError::InvalidInput(format!(
                    "each plan step must be at most {MAX_TEXT} characters"
                )));
            }
        }
        // At most one step is the one being worked on. Zero is valid: a plan
        // recorded before the first step starts ("add one item per concrete
        // step before you start") has nothing in progress yet, and refusing
        // that call cost the model a round trip to learn a rule the schema
        // could not state.
        let active = parsed
            .items
            .iter()
            .filter(|item| !item.text.trim().is_empty() && item.status == PlanStatus::InProgress)
            .count();
        if active > 1 {
            return Err(ToolError::InvalidInput(
                "a plan may keep at most one step in_progress; mark the others pending".to_string(),
            ));
        }

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
        tool.run(&serde_json::json!({"items": [{"text": "a", "status": "in_progress"}]}))
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
    fn blank_items_are_dropped_without_breaking_the_plan() {
        let tool = tool();
        tool.run(&serde_json::json!({
            "items": [
                {"text": "   ", "status": "pending"},
                {"text": "build", "status": "in_progress"},
                {"text": "verify", "status": "pending"}
            ]
        }))
        .expect("run");
        let items = tool.store().items();
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|item| !item.text.trim().is_empty()));
        assert_eq!(
            items
                .iter()
                .filter(|item| item.status == PlanStatus::InProgress)
                .count(),
            1
        );
    }

    #[test]
    fn oversized_plan_is_rejected_without_truncation() {
        let tool = tool();
        let items: Vec<Value> = (0..MAX_ITEMS + 1)
            .map(|i| {
                serde_json::json!({
                    "text": format!("step {i}"),
                    "status": if i == 0 { "in_progress" } else { "pending" }
                })
            })
            .collect();
        assert!(tool.run(&serde_json::json!({"items": items})).is_err());
        assert!(tool.store().items().is_empty());
    }

    #[test]
    fn oversized_step_text_is_rejected_without_truncation() {
        let tool = tool();
        let long = "x".repeat(MAX_TEXT + 1);
        assert!(tool
            .run(&serde_json::json!({
                "items": [{"text": long, "status": "in_progress"}]
            }))
            .is_err());
        assert!(tool.store().items().is_empty());
    }

    /// A plan recorded before the first step starts has nothing in progress;
    /// two steps in progress would give the panel two focuses.
    #[test]
    fn a_recorded_plan_may_have_no_active_step_but_never_two() {
        let tool = tool();
        let no_active = serde_json::json!({"items": [{"text": "a", "status": "pending"}]});
        tool.run(&no_active)
            .expect("a plan recorded before it starts is valid");
        assert_eq!(tool.store().items().len(), 1);

        let two_active = serde_json::json!({"items": [
            {"text": "a", "status": "in_progress"},
            {"text": "b", "status": "in_progress"}
        ]});
        assert!(tool.run(&two_active).is_err());
        assert_eq!(
            tool.store().items().len(),
            1,
            "the rejected call left the stored plan untouched"
        );
    }

    /// Blank steps are dropped by the store, so they must not consume one of
    /// the 100 slots a real step may use.
    #[test]
    fn blank_steps_do_not_consume_the_step_cap() {
        let tool = tool();
        let mut items: Vec<Value> = (0..MAX_ITEMS)
            .map(|i| serde_json::json!({"text": format!("step {i}"), "status": "pending"}))
            .collect();
        items.push(serde_json::json!({"text": "   ", "status": "pending"}));
        tool.run(&serde_json::json!({ "items": items }))
            .expect("a blank step rides along without taking a slot");
        assert_eq!(tool.store().items().len(), MAX_ITEMS);
    }
    /// Text is trimmed before it is stored, and the length bound is measured
    /// after trimming: surrounding whitespace neither survives nor counts
    /// against the 500-character budget.
    ///
    /// This test used to pin truncation of an oversized step. The tool now
    /// refuses one instead (see `oversized_step_text_is_rejected_without_truncation`),
    /// because silently shortening the model's own step text made the stored
    /// plan disagree with what the model believed it had recorded.
    #[test]
    fn item_text_is_trimmed_before_it_is_stored() {
        let tool = tool();
        let padded = format!("  {}  ", "x".repeat(MAX_TEXT));
        tool.run(&serde_json::json!({
            "items": [{"text": padded, "status": "in_progress"}]
        }))
        .expect("a step at the bound, once trimmed, is accepted");
        let items = tool.store().items();
        assert_eq!(items[0].text, "x".repeat(MAX_TEXT));
        assert_eq!(items[0].text.chars().count(), MAX_TEXT);
    }

    /// The description carries the reference todo guidance, with BSH's tool
    /// name and single-active-item rule. This is the behavioural instruction
    /// the model reads on every request, so it is pinned exactly.
    #[test]
    fn the_description_carries_the_todo_guidance() {
        let description = tool().description();
        assert!(description.starts_with(
            "Record and update a task list to plan multi-step work and show progress; skip it for trivial single-step tasks. Add one item per concrete step before you start."
        ));
        assert!(description.contains("While work remains, keep at most one item `in_progress`"));
        assert!(description.contains("Mark each item `completed` as soon as it is done"));
        assert!(description.contains("Pass `items` as the COMPLETE list of steps"));
        assert!(!description.contains("todo_write"));
    }

    #[test]
    fn schema_declares_the_items_contract() {
        let schema = tool().json_schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"][0], "items");
        assert_eq!(schema["properties"]["items"]["maxItems"], MAX_ITEMS);
        assert_eq!(
            schema["properties"]["items"]["items"]["properties"]["text"]["maxLength"],
            MAX_TEXT
        );
        assert_eq!(schema["additionalProperties"], false);
        let status = &schema["properties"]["items"]["items"]["properties"]["status"];
        assert_eq!(status["enum"][1], "in_progress");
    }
}
