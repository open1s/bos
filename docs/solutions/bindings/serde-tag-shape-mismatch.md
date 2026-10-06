---
module: react
tags: [bindings, serde, python, nodejs, content, contract, testing, bug]
problem_type: bug
---

# Hand-built binding JSON must match the Rust enum serde tag

## Problem

The two bindings build message content as plain JSON and hand it to the Rust
backend. Python's `Binary.to_dict` emitted an adjacently tagged source,
`{"type": "url", "data": ...}`, while `BinarySource` in
`crates/react/src/llm/types.rs` is externally tagged: serde expects
`{"url": ...}` or `{"base64": ...}`. The two cannot be mixed, and neither
`parity.test.js` (which compares member names) nor any content test noticed.

## Why it failed silently

`Content::from(String)` tries `Vec<ContentPart>`, then `ContentPart`, then
falls back to `Content::Text(s)`. That fallback is friendly for an ordinary
prompt string, but it turns a decode error into data loss: every image or
audio clip sent from Python was dropped and the raw JSON was sent to the model
as text. Nothing raised, so the only symptom was a worse answer.

The JS binding happened to emit the externally tagged shape, so the bug was
invisible from JS and surfaced only when the Python path was tested.

## Fix

- `crates/nbos/nbos/content.py` emits `{"url": ...}` / `{"base64": ...}`.
- Rust `Content::image(url)` also built a base64 binary typed `image url`
  (with a space), which `is_image()` rejects, so `Message::user_image` never
  produced an image part at all; it now builds a URL image typed `image/jpeg`.
- `ContentPart.image` aligns on both sides: `image(url, name=None)`, typed
  `image/jpeg` (the inert Python `detail` parameter is gone).

## Guard

- `crates/react/tests/content_serialization_test.rs` gains
  `test_adjacently_tagged_source_does_not_decode`, which pins the text fallback
  so the failure mode cannot quietly change, plus
  `test_shared_multimodal_fixture_decodes`.
- One fixture, `crates/react/tests/fixtures/content_parts.json`, is read by the
  Rust, Python, and JS content tests. Each suite serializes its own content and
  compares it against the fixture, so a one-sided change fails all three.

## Lesson

When a binding serializes a Rust enum by hand, mirror the serde
representation exactly. Prefer deriving the wire type once and reusing it. Treat
a lenient `From<String>` fallback as a trap: it must never be the thing that
hides a contract mismatch.