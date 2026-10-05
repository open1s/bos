---
module: jsbos
tags: [bindings, parity, python, nodejs, testing, contract]
problem_type: best-practice
---

# Keep the Python and JavaScript binding surfaces in lockstep

## Problem

Native capabilities live in the Rust crates, but the user-facing API is written
twice, in crates/jsbos/index.js and crates/nbos/nbos/core.py. Nothing connected
the two, so the surfaces drifted repeatedly and silently:

- tool listing was sync-only in both crates, hiding every binding-registered
  tool;
- Python had no performance metrics while JS did;
- Python's session export produced an AgentState shape that its own import
  rejected, while JS produced an AgentSession;
- MCP prompt listing returned bare names in Python and entries in JS.

Each was only visible by auditing both languages together.

## Guard

crates/jsbos/test/parity.test.js statically parses both source files, extracts
the public members of SessionManager and the high-level agent, normalizes the
naming conventions (camelCase to snake_case; an optional with_ builder prefix is
stripped on either side, so with_config matches withConfig and with_model
matches model), and fails on any difference not on an explicit allowlist.

It runs inside the existing yarn test (ava) job and needs no Python runtime,
because it reads the Python source instead of importing it.

## Contract

The high-level agent surface is now fully mirrored, so its allowlist is empty.
Getting there closed real gaps and reconciled names:

- Python gained with_config, with_circuit_breaker, with_rate_limit, stop,
  is_running and stream_collect.
- JS gained chat and plugins as alias/bulk forms, and Python gained with_system,
  so every alias now exists on both sides.
- JS skillsFromDir was renamed to skillsDir, matching the noun form used by
  baseUrl and model and aligning with Python's with_skills_dir.

Only SessionManager keeps a documented difference: the JS import alias. The
contract checks presence, not arity, so a getter and a similarly named setter
collapse into one entry.
