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

crates/jsbos/test/parity.test.js statically parses crates/jsbos/index.js and
every module under crates/nbos/nbos/, extracts the public members of each paired
class, normalizes the naming conventions (camelCase to snake_case with acronym
runs collapsed, so toJSON matches to_json; an optional with_ builder prefix is
stripped on either side, so with_config matches withConfig and with_model
matches model), and fails on any difference not on an explicit allowlist.

The extractors are covered by their own tests, because a broken one fails open —
it simply returns fewer members and every surface looks like it matches:

- JS accepts static/async/get/set modifiers. Static methods used to be invisible,
  which hid half of every class's static surface.
- Python ends a class body at the first column-0 statement, so module-level
  helpers (and the functions nested inside them) are not attributed to the last
  class in a file.

It runs inside the existing ava job and needs no Python runtime, because it reads
the Python source instead of importing it.

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

The guard also covers the multimodal wire types (Binary, ContentPart, Content),
the tool definition types (ToolDef, ToolResult), and ToolRegistry. The value
types keep two documented differences: cross-idiom serialization (Python's
to_dict versus JS's toJSON, and JS's toString alongside Python's to_json) and the
JS-only static ToolResult.fromResult helper. The bus/query/caller classes remain
uncovered: their JS surfaces are richer, so reconciling them is tracked as
follow-up work rather than locked in.

Config was reconciled by giving Python the JS fluent surface (file, directory,
inline, discover, reset, load, reload, get, is_loaded, to_json, the
global_model/model/base_url/api_key/bus accessors, and the from_file,
from_directory and from_inline constructors). The original add_file,
add_directory, add_inline, load_sync and reload_sync names remain as documented
aliases, which is the only allowlisted difference.

The BrainOS facade is guarded too. Python gained create, start, stop,
is_started, config and create_bus to match the JS entry point. BusManager
gained start and stop along the way, which also fixed a leak: its __aexit__
used to return without closing the underlying bus.

The Publisher and Subscriber wrappers now share one vocabulary:
publish/publish_text/publish_json and recv/recv_json/run/run_json/next, plus
topic on both. JS keeps text/json as deprecated aliases of publishText and
publishJson; Python keeps recv_with_timeout_ms and recv_json_with_timeout_ms
as deprecated aliases of recv and recv_json. Both sets are allowlisted, so
the guard still fails on any new drift. Fixing the pair closed a latent JS
bug: SubscriberWrapper.recvJson() with no timeout called a native recvJson
that does not exist.

Closing the ToolRegistry gap added unregister, list_tool_defs, list_by_category,
filter and to_json to Python. list_by_category filters on an optional category
attribute and returns an empty list until Python's ToolDef gains a category
field; the JS side reads it from BaseTool metadata.
