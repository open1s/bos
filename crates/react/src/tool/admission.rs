//! Admission control for the tool definitions a request advertises.
//!
//! A request's tool array is part of every step's prompt prefix, so its size is
//! paid again on every model call. A host can mark a definition deferred (see
//! [`crate::tool::registry::Tool::deferred`]) and let `find_tool` reveal it on
//! demand, but NOTHING bounded the definitions that stay in the array: a wide
//! registry - an extension platform, a preset with every bundle on, a machine
//! with a dozen local tools - made each request carry all of them.
//!
//! This module is the missing bound. It is a pure function of the definitions,
//! the policy and the names that must never be withheld, so the request's array
//! and `find_tool`'s catalogue can be derived from the same decision instead of
//! from two guesses at it.
//!
//! Two rules keep the bound safe:
//!
//! - A required definition is never withheld, whatever it costs. The standing
//!   prompt may name it, and a safety tool that silently disappears is worse
//!   than a larger request.
//! - Nothing is dropped silently. Every withheld definition is reported with
//!   its size and the reason, and the caller adds the discovery search in its
//!   place, so the model always has a way back to what was withheld.

use std::collections::BTreeSet;

use crate::llm::LlmTool;

/// Bytes the initial advertisement leaves for a later `find_tool` reveal.
///
/// The initial array and the discovered definitions spend one per-turn budget;
/// holding a slice back is what makes discovery usable instead of a way back
/// that is always refused. Eight kilobytes is two full-size discovery results,
/// which is more than one turn normally asks for.
pub const DEFAULT_DISCOVERY_RESERVE: usize = 8 * 1024;

/// Why a definition was withheld from the initial request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithheldReason {
    /// One definition alone is over the per-definition ceiling.
    TooLarge,
    /// The cumulative initial budget has no room left.
    OverBudget,
}

/// One definition the initial request does not carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithheldTool {
    /// Registered tool name.
    pub name: String,
    /// Serialized definition bytes it would have spent.
    pub bytes: usize,
    /// Why it is not in the array.
    pub reason: WithheldReason,
}

/// The bound applied to one initial tool array.
#[derive(Debug, Clone)]
pub struct AdmissionPolicy {
    /// Per-turn cumulative byte ceiling. Zero admits every definition and
    /// keeps the historical behaviour.
    pub budget: usize,
    /// Ceiling for one optional definition. Zero disables the per-tool check.
    /// A required definition is never refused by it.
    pub per_tool: usize,
    /// Names that must never be withheld.
    pub required: BTreeSet<String>,
    /// Bytes held back from this advertisement for discovery.
    pub reserve: usize,
}

impl AdmissionPolicy {
    /// A policy under `budget` bytes with the default discovery reserve.
    pub fn new(budget: usize, per_tool: usize) -> Self {
        Self {
            budget,
            per_tool,
            required: BTreeSet::new(),
            reserve: DEFAULT_DISCOVERY_RESERVE,
        }
    }

    /// Never withhold `name`.
    pub fn require(mut self, name: impl Into<String>) -> Self {
        self.required.insert(name.into());
        self
    }

    /// Hold back bytes for discovery, capped at the budget itself.
    pub fn with_reserve(mut self, bytes: usize) -> Self {
        self.reserve = bytes;
        self
    }

    /// Bytes the initial advertisement may spend; zero when the budget is off.
    pub fn initial_limit(&self) -> usize {
        if self.budget == 0 {
            0
        } else {
            self.budget.saturating_sub(self.reserve)
        }
    }
}

impl Default for AdmissionPolicy {
    fn default() -> Self {
        Self::new(0, 0)
    }
}

/// What one admission did to the array.
#[derive(Debug, Clone, Default)]
pub struct Admission {
    /// The definitions the request carries, name-sorted. Required definitions
    /// keep their place even when the budget is overdrawn.
    pub tools: Vec<LlmTool>,
    /// Definition bytes the admitted array spends.
    pub bytes: usize,
    /// Bytes the required definitions alone spend.
    pub required_bytes: usize,
    /// Definitions the initial request does not carry, name-sorted.
    pub withheld: Vec<WithheldTool>,
    /// The initial ceiling that was applied; zero when the budget is off.
    pub limit: usize,
    /// Whether the required definitions alone exceed `limit`. The array still
    /// carries them; optional definitions are all withheld.
    pub over_budget: bool,
}

impl Admission {
    /// Whether anything left the initial request.
    pub fn has_withheld(&self) -> bool {
        !self.withheld.is_empty()
    }

    /// The withheld names, sorted.
    pub fn withheld_names(&self) -> Vec<String> {
        self.withheld.iter().map(|tool| tool.name.clone()).collect()
    }

    /// The withheld names as a set, for a catalogue or a telemetry sink.
    pub fn withheld_set(&self) -> BTreeSet<String> {
        self.withheld.iter().map(|tool| tool.name.clone()).collect()
    }

    /// The total bytes of every definition the array left out.
    pub fn withheld_bytes(&self) -> usize {
        self.withheld.iter().map(|tool| tool.bytes).sum()
    }
}

/// Admit the definitions a request may carry.
///
/// The result is deterministic: candidates are visited in name order, so the
/// same registry always yields the same array and the provider's cached prefix
/// cannot reshuffle because a tool was registered in a different order.
///
/// A definition that does not fit is skipped, and the scan continues: a single
/// oversized specialist does not cost every smaller definition behind it.
pub fn admit(tools: Vec<LlmTool>, policy: &AdmissionPolicy) -> Admission {
    let mut candidates = tools;
    candidates.sort_by(|a, b| a.name.cmp(&b.name));
    candidates.dedup_by(|a, b| a.name == b.name);

    if policy.budget == 0 {
        let bytes = candidates.iter().map(LlmTool::schema_bytes).sum();
        return Admission {
            tools: candidates,
            bytes,
            required_bytes: bytes,
            withheld: Vec::new(),
            limit: 0,
            over_budget: false,
        };
    }

    let limit = policy.initial_limit();
    let mut admitted: Vec<LlmTool> = Vec::with_capacity(candidates.len());
    let mut withheld: Vec<WithheldTool> = Vec::new();
    let mut bytes = 0usize;
    let mut required_bytes = 0usize;

    // Required definitions are priced first: the budget cannot buy an optional
    // definition with room a required one needs.
    for tool in candidates
        .iter()
        .filter(|tool| policy.required.contains(&tool.name))
    {
        let size = tool.schema_bytes();
        required_bytes = required_bytes.saturating_add(size);
        bytes = bytes.saturating_add(size);
        admitted.push(tool.clone());
    }

    for tool in candidates
        .iter()
        .filter(|tool| !policy.required.contains(&tool.name))
    {
        let size = tool.schema_bytes();
        if policy.per_tool != 0 && size > policy.per_tool {
            withheld.push(WithheldTool {
                name: tool.name.clone(),
                bytes: size,
                reason: WithheldReason::TooLarge,
            });
            continue;
        }
        if bytes.saturating_add(size) > limit {
            withheld.push(WithheldTool {
                name: tool.name.clone(),
                bytes: size,
                reason: WithheldReason::OverBudget,
            });
            continue;
        }
        bytes = bytes.saturating_add(size);
        admitted.push(tool.clone());
    }

    // The array is the provider's cached prefix, so its order is the set's,
    // not the order the required pass happened to visit.
    admitted.sort_by(|a, b| a.name.cmp(&b.name));
    Admission {
        tools: admitted,
        bytes,
        required_bytes,
        withheld,
        limit,
        over_budget: required_bytes > limit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, schema_bytes: usize) -> LlmTool {
        // A description of the requested size keeps each case readable; the
        // definition's price is the sum of its parts (see LlmTool::schema_bytes).
        LlmTool::function(
            name,
            "d".repeat(schema_bytes),
            serde_json::json!({"type": "object"}),
        )
    }

    #[test]
    fn a_registry_larger_than_the_budget_is_bounded() {
        let tools: Vec<LlmTool> = (0..100)
            .map(|index| tool(&format!("tool_{index:03}"), 1_000))
            .collect();
        let policy = AdmissionPolicy::new(10_000, 1_000_000).with_reserve(0);
        let admission = admit(tools.clone(), &policy);

        assert!(
            admission.bytes <= admission.limit,
            "admitted {} bytes over a {} byte limit",
            admission.bytes,
            admission.limit
        );
        assert!(
            !admission.withheld.is_empty(),
            "the rest is reported, not dropped"
        );
        assert_eq!(
            admission.withheld_bytes(),
            tools.iter().map(LlmTool::schema_bytes).sum::<usize>() - admission.bytes,
            "every withheld byte is accounted"
        );
        assert_eq!(
            admission.tools.len() + admission.withheld.len(),
            tools.len(),
            "every definition is either admitted or reported"
        );
        assert!(admission.bytes > 0, "the budget admits what fits");
    }

    #[test]
    fn a_required_definition_is_never_withheld() {
        let tools = vec![
            tool("bash", 4_000),
            tool("optional_small", 1_000),
            tool("optional_big", 9_000),
        ];
        let policy = AdmissionPolicy::new(6_000, 2_000)
            .with_reserve(0)
            .require("bash");
        let admission = admit(tools, &policy);

        let names: Vec<&str> = admission.tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"bash"),
            "the required tool stays: {names:?}"
        );
        assert!(
            !names.contains(&"optional_big"),
            "an optional tool over the per-tool ceiling leaves: {names:?}"
        );
        assert_eq!(admission.withheld_names(), vec!["optional_big".to_string()]);
        assert!(
            admission.required_bytes >= 4_000,
            "the required definition's own bytes are counted"
        );
        assert!(!admission.over_budget);
    }

    #[test]
    fn required_definitions_over_the_budget_are_still_sent_and_reported() {
        let tools = vec![
            tool("must_a", 4_000),
            tool("must_b", 4_000),
            tool("extra", 500),
        ];
        let policy = AdmissionPolicy::new(5_000, 10_000)
            .with_reserve(0)
            .require("must_a")
            .require("must_b");
        let admission = admit(tools, &policy);

        assert_eq!(admission.tools.len(), 2, "both required definitions stay");
        assert!(admission.over_budget, "the overdraft is reported");
        assert_eq!(admission.withheld_names(), vec!["extra".to_string()]);
    }

    #[test]
    fn a_smaller_definition_behind_an_oversized_one_is_admitted() {
        let tools = vec![
            tool("a_huge", 50_000),
            tool("b_small", 1_000),
            tool("c_small", 1_000),
        ];
        let policy = AdmissionPolicy::new(10_000, 1_000_000).with_reserve(0);
        let admission = admit(tools, &policy);

        let names: Vec<&str> = admission.tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["b_small", "c_small"]);
        assert_eq!(
            admission.withheld_names(),
            vec!["a_huge".to_string()],
            "the scan does not stop at the first definition that does not fit"
        );
    }

    #[test]
    fn the_reserve_is_held_back_for_discovery() {
        let policy = AdmissionPolicy::new(64 * 1024, 16 * 1024);
        assert_eq!(
            policy.initial_limit(),
            64 * 1024 - DEFAULT_DISCOVERY_RESERVE
        );
        assert_eq!(
            AdmissionPolicy::new(4 * 1024, 0).initial_limit(),
            0,
            "a reserve larger than the budget leaves nothing, not a negative limit"
        );
    }

    #[test]
    fn zero_budget_admits_everything() {
        let tools = vec![tool("a", 50_000), tool("b", 50_000)];
        let admission = admit(tools, &AdmissionPolicy::default());
        assert_eq!(admission.tools.len(), 2);
        assert!(!admission.has_withheld());
        assert_eq!(admission.limit, 0, "zero means the budget is off");
    }

    #[test]
    fn the_array_is_name_sorted_whatever_the_registration_order() {
        let forward = vec![tool("alpha", 100), tool("mike", 100), tool("zulu", 100)];
        let mut reverse = forward.clone();
        reverse.reverse();
        let policy = AdmissionPolicy::new(10_000, 10_000);

        assert_eq!(
            admit(forward, &policy)
                .tools
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>(),
            admit(reverse, &policy)
                .tools
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>(),
            "the cached prefix cannot depend on registration order"
        );
    }

    #[test]
    fn a_repeat_of_one_name_is_admitted_once() {
        let tools = vec![tool("echo", 100), tool("echo", 200), tool("zulu", 100)];
        let admission = admit(tools, &AdmissionPolicy::new(10_000, 10_000));
        assert_eq!(
            admission.tools.iter().filter(|t| t.name == "echo").count(),
            1
        );
    }
}
