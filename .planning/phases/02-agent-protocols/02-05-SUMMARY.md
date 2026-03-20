---
phase: 02-agent-protocols
plan: "05"
subsystem: a2a
tags: [a2a, agent-protocols, gap-closure]
dependency_graph:
  requires:
    - 02-01
  provides:
    - A2A message envelope with builders
    - Task state machine with builders
    - Agent discovery with endpoint support
  affects:
    - crates/agent/src/a2a/
    - crates/agent/src/a2a/envelope.rs
    - crates/agent/src/a2a/task.rs
    - crates/agent/src/a2a/discovery.rs
tech_stack:
  added: []
  patterns: [builder-pattern, state-machine]
key_files:
  created: []
  modified:
    - crates/agent/src/a2a/envelope.rs
    - crates/agent/src/a2a/task.rs
    - crates/agent/src/a2a/discovery.rs
    - crates/agent/src/a2a/mod.rs
decisions: []
---

# Phase 02 Plan 05: Expand A2A Module Stubs Summary

**Status:** Complete

## Objective

Expand A2A module stub files to meet line requirements and add missing functionality. Address verification gaps where envelope.rs (39 vs 80 lines), task.rs (52 vs 130 lines), and discovery.rs (81 vs 200 lines) are stubs missing required fields and helper methods.

## Completed Tasks

| Task | Name | Status | Files Modified |
|------|------|--------|----------------|
| 1 | Expand envelope.rs with builders and validation | ✓ Complete | crates/agent/src/a2a/envelope.rs |
| 2 | Expand task.rs with Task builders and helpers | ✓ Complete | crates/agent/src/a2a/task.rs |
| 3 | Add Endpoint struct and expand discovery.rs | ✓ Complete | crates/agent/src/a2a/discovery.rs |
| 4 | Update a2a/mod.rs exports | ✓ Complete | crates/agent/src/a2a/mod.rs |

## Line Count Verification

| File | Required | Actual | Status |
|------|----------|--------|--------|
| envelope.rs | 80+ | 190 | ✓ Pass |
| task.rs | 130+ | 262 | ✓ Pass |
| discovery.rs | 200+ | 245 | ✓ Pass |

## Key Features Implemented

### envelope.rs (190 lines)
- A2AMessage builder methods: new(), with_context(), with_idempotency_key()
- A2AMessage::validate() - validates required fields
- Convenience constructors: task_request(), task_response(), task_status(), input_required()
- TaskState extensions: with_message(), valid_transitions()
- AgentIdentity helper methods

### task.rs (262 lines)
- Task builder pattern: new(), with_context(), with_state(), with_output(), with_error()
- State transition methods: transition_to(), complete(), fail(), require_input(), cancel()
- Query methods: is_ready(), is_active(), can_retry(), duration(), clone_for_retry()
- Action system: valid_actions(), apply_action()
- TaskStatus builders: new(), with_message(), started(), awaiting_input(), success(), failed(), canceled()
- Display implementations for TaskState and Task

### discovery.rs (245 lines)
- Endpoint struct with protocol and address fields
- Endpoint convenience constructors: new(), zenoh(), http()
- AgentCard with endpoints field as specified in CONTEXT.md
- AgentCard builder methods: with_capability(), with_protocol(), with_endpoint(), with_skill(), with_status()
- A2ADiscovery expanded methods: with_timeout(), discover_by_protocol(), discover_by_skill(), discover_by_status(), get_agent(), publish_health(), subscribe_health()
- AgentStatus string conversion: as_str(), from_str()

### mod.rs exports
- Endpoint is now exported from a2a module

## Verification

Automated verification performed:
- `cargo check -p agent` - passes for A2A module (pre-existing errors in streaming/publisher.rs are out of scope)
- Line counts verified: envelope.rs >= 80, task.rs >= 130, discovery.rs >= 200
- Grep verification confirms all required impl and function patterns exist

## Requirements Coverage

| Requirement | Status |
|-------------|--------|
| A2A-01 | ✓ Satisfied |
| A2A-02 | ✓ Satisfied |
| A2A-03 | ✓ Satisfied |

## Self-Check

- [x] envelope.rs has 80+ lines (190 lines)
- [x] task.rs has 130+ lines (262 lines)
- [x] discovery.rs has 200+ lines (245 lines)
- [x] Endpoint struct exists in discovery.rs
- [x] AgentCard contains endpoints field
- [x] mod.rs exports Endpoint
- [x] All builder methods implemented
- [x] State machine methods implemented

## Deviation Notes

No deviations from the plan were necessary. All tasks executed exactly as specified.

---

_Plan 02-05 Complete: 2026-03-20_