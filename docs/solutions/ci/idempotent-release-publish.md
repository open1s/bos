---
module: jsbos
tags: [ci, release, npm, pypi, idempotency, github-actions]
problem_type: best-practice
---

# Make release publishing idempotent

## Problem

A version commit (subject `x.y.z`) and its `v*` tag both trigger the jsbos
pipeline for the same commit. Both runs pass the "Detect a version commit"
gate, so both reach the Publish job. The second attempts to publish an
already-published version and fails the run, turning a successful release red.
A manual re-run has the same effect. The duplicate is also expensive: the full
native build matrix runs twice.

## Fix

The npm Publish step reads the package name and version from package.json and
exits early when that version already exists on the registry, so a second
trigger or a re-run is a no-op instead of a failure. The PyPI publish uses
`skip-existing: true` for the same reason.

## Note

This is separate from the 2.4.2 npm failure: no npm artifact existed for that
version, so the registry 404 was a publish-permission or registry problem, not
a duplicate. Idempotency removes the duplicate-trigger failure class; a first
publish that the registry rejects still needs to be understood on its own.