---
module: jsbos
tags: [napi-rs, threadsafe-function, nodejs, bindings, event-loop, lifecycle]
problem_type: bug
---

# napi ThreadsafeFunctions must be weak when they are run-scoped

## Symptom

A Node.js script that registered a tool, hook, or plugin through `jsbos` printed
its final output and then **hung**: the process never exited. Running the test
suite produced `Failed to exit when running test/...` and a two-minute timeout,
even though every test passed.

## Cause

Every JavaScript callback that crosses into Rust arrives as a
`napi::bindgen_prelude::ThreadsafeFunction`. By default a TSF holds a strong
reference to the event loop, so as long as the native agent keeps the TSF alive
(which it does for the whole agent lifetime) Node refuses to exit.

`add_tool`, `register_hook`, `register_plugin`, and `stream` all stored their
callbacks for later invocation, so any one of them pinned the loop. napi-rs v3
exposes no public `unref` method.

## Fix

napi-rs makes the reference strength a const generic on the type:

```rust
ThreadsafeFunction<
  T,                          // value passed to JS
  napi::Unknown<'static>,     // JS return
  T,                          // tuple of arguments
  napi::Status,               // error strategy
  true,                       // CalleeHandled
  true,                       // Weak  <-- stop pinning the event loop
>
```

Use this weak form for every **run-scoped** callback: tools, hooks, plugins, and
streaming handlers. A weak TSF still delivers calls normally; it only declines to
keep the process alive when nothing else is pending.

Do **not** blindly convert event-driven handlers (callers, subscribers). There,
pinning the loop is often the intended lifetime.

## Guard

The `crates/jsbos` suite registers a tool, a plugin, and a hook; because ava must
exit on its own, any reintroduced strong TSF fails the run with "Failed to exit".
