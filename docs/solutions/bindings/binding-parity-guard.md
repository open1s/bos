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

## Allowlist

Known, intentional differences are listed in the test:

- JS-only: the import back-compat alias, plus the system and skillsFromDir
  names (aliases of the prompt setter and the skills-directory setter).
- Python-only: chat (an ask alias) and the plugins / skills_dir names.

Both bindings now expose the same config surface: Python gained with_config,
with_circuit_breaker and with_rate_limit to match JS withConfig, circuitBreaker
and rateLimit. The contract checks presence, not arity, so a getter and a
similarly named setter collapse into one entry.

When one binding gains a member, the suite fails until the other side (or the
allowlist) is updated, so divergence becomes a deliberate decision rather than
an accident. The allowlist is now down to alias and naming choices; every
capability gap it once tracked (Python stop, isRunning, streamCollect,
with_config, granular resilience) has been closed.
