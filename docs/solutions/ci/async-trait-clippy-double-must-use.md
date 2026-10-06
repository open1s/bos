---
module: react
tags: [ci, clippy, async-trait, dependencies, linting, toolchain]
problem_type: bug
---

# async-trait 0.1.89 trips clippy 1.99 double_must_use under -D warnings

## Problem

CI lint jobs run on stable Rust, which moved ahead of the local toolchain
(1.99.0 vs 1.98.1 locally). Clippy 1.99 added the double_must_use lint, and
async-trait 0.1.89 trips it on every async trait method: the macro pushes
must_use onto the desugared method, whose return type (a pinned boxed Future)
is already considered must_use.

Every async_trait site fails, so the error looks like a broken trait
definition and names a file the repo owns:

```
error: this function has a #[must_use] attribute with no message, but
returns a type already considered as #[must_use]
  --> crates/react/src/tool/registry.rs:39:1
39 |   #[async_trait]
   |   ^^^^^^^^^^^^^^
note: this error originates in the attribute macro `async_trait`
```

Nothing in the repo changed, which makes it look like a code regression.

## Diagnosis

Two things together are the tell:

- the error spans several unrelated files, all of them async_trait
  attributes;
- the local toolchain does not reproduce it.

Diffing the two cached versions of the macro source settles it:

```
async-trait-0.1.89/src/expand.rs: method.attrs.push(parse_quote!(#[must_use]));
async-trait-0.1.92/src/expand.rs: (line gone)
```

## Solution

Update async-trait to 0.1.92, which dropped the redundant attribute. The
workspace dependency is pinned at 0.1.92 with a comment in the root Cargo.toml,
and Cargo.lock carries it for every consumer.

The general lesson: when a lint error appears only in CI and points at a macro
invocation the repo does not own, check whether the toolchain moved first, then
whether the macro has a newer version that already fixed it, before
suppressing the lint anywhere.

## Related

The lint failure had masked the Test step for weeks, because a workflow job
stops at its first failing step; a flaky resource watch test surfaced only
after the lint was fixed. Fixing a long-standing red step can expose buried
ones, so expect to fix more than one thing.