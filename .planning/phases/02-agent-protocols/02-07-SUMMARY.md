---
phase: 02-agent-protocols
plan: "07"
subsystem: agent
tags: [a2a, zenoh, topics, bus]
dependency_graph:
  requires:
    - 02-01
    - 02-04
  provides:
    - A2A-01
    - A2A-02
    - A2A-03
    - A2A-04
    - STRM-02
    - STRM-03
  affects:
    - crates/agent/src/a2a/client.rs
    - crates/agent/src/streaming/publisher.rs
tech_stack:
  added: []
  patterns:
    - Topic path constants via helper functions
    - Bus crate PublisherWrapper abstraction
    - Backward compatibility type alias
key_files:
  created:
    - crates/agent/src/a2a/topics.rs
  modified:
    - crates/agent/src/a2a/client.rs
    - crates/agent/src/a2a/mod.rs
    - crates/agent/src/streaming/publisher.rs
    - crates/agent/src/streaming/mod.rs
decisions: []
metrics:
  duration: "2026-03-20T04:30:00Z"
  completed: "2026-03-20"
  task_count: 3
  file_count: 5
---

# Phase 02 Plan 07: Fix A2A Topic Paths and Bus Crate Integration Summary

One-liner: Fixed A2A topic paths to match specification and refactored streaming publisher to use bus crate wrapper.

## Objective

Fix verification gaps where:
- A2A client topic paths don't match 02-CONTEXT.md specification
- Streaming publisher uses zenoh Session directly instead of bus crate PublisherWrapper

## Tasks Completed

| Task | Name | Status | Commit |
|------|------|--------|--------|
| 1 | Fix A2A client topic paths | ✅ | 7cffdc0 |
| 2 | Refactor streaming publisher to use bus crate | ✅ | 7cffdc0 |
| 3 | Add topic constant documentation to A2A module | ✅ | 7cffdc0 |

## Key Changes

### Task 1: Fixed A2A Client Topic Paths

**Files modified:** `crates/agent/src/a2a/client.rs`

Changes made:
- Fixed `delegate_task()` topic path from `agent/{}/tasks` to `agent/{}/tasks/incoming`
- Fixed `poll_status()` topic path from `agent/{}/status/{}` to `agent/{}/tasks/{}/status`
- Added documentation comment showing topic structure from 02-CONTEXT.md

### Task 2: Refactored Streaming Publisher

**Files modified:** `crates/agent/src/streaming/publisher.rs`, `crates/agent/src/streaming/mod.rs`

Changes made:
- Renamed `PublisherWrapper` to `TokenPublisherWrapper`
- Added `bus_publisher: BusPublisher` field using `bus::PublisherWrapper`
- Updated constructor to initialize bus publisher
- Updated `flush_batch()` to use `bus_publisher.publish_raw()` instead of direct zenoh
- Added backward compatibility type alias: `pub type PublisherWrapper = TokenPublisherWrapper;`

### Task 3: Added Topics Module

**Files created:** `crates/agent/src/a2a/topics.rs`  
**Files modified:** `crates/agent/src/a2a/mod.rs`, `crates/agent/src/a2a/client.rs`

Changes made:
- Added `pub mod topics` with helper functions:
  - `tasks_incoming(agent_id)` → `agent/{agent_id}/tasks/incoming`
  - `task_status(agent_id, task_id)` → `agent/{agent_id}/tasks/{task_id}/status`
  - `response(agent_id, correlation_id)` → `agent/{agent_id}/responses/{correlation_id}`
  - `DISCOVERY_ANNOUNCE` constant
  - `health(agent_id)` → `agent/discovery/health/{agent_id}`
- Updated client.rs to use topics module functions instead of inline format strings

## Verification

Verified changes:
- ✅ `grep "agent/{}/tasks/incoming" crates/agent/src/a2a/client.rs` - Found in docstring and usage
- ✅ `grep "agent/{}/tasks/{}/status" crates/agent/src/a2a/client.rs` - Found in docstring and usage
- ✅ `grep "use bus::PublisherWrapper" crates/agent/src/streaming/publisher.rs` - Found
- ✅ `grep "pub struct TokenPublisherWrapper" crates/agent/src/streaming/publisher.rs` - Found
- ✅ `grep "bus_publisher: BusPublisher" crates/agent/src/streaming/publisher.rs` - Found
- ✅ `! grep "session.declare_publisher" crates/agent/src/streaming/publisher.rs` - Not found (removed)
- ✅ `grep "pub mod topics" crates/agent/src/a2a/mod.rs` - Found
- ✅ `grep "use super::topics" crates/agent/src/a2a/client.rs` - Found

## Build Status

Build completes with only pre-existing error in `discovery.rs` (unrelated to these changes).

## Deviations from Plan

None - plan executed exactly as written.

## Self-Check

- [x] Files created exist: `crates/agent/src/a2a/topics.rs`
- [x] Files modified exist: all 4 files listed above
- [x] Commit exists: `7cffdc0`

## Self-Check: PASSED