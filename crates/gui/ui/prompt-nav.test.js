/* Node test for the composer's prompt-history state machine.
 * Run: node crates/gui/ui/prompt-nav.test.js
 */
"use strict";
const assert = require("assert");
const PromptNav = require("./prompt-nav.js");

const H = ["first question", "second question", "third question"];

// Engage from scratch: first ↑ recalls the newest prompt.
{
  let step = PromptNav.up(H, null, "");
  assert.strictEqual(step.text, "third question");
  assert.strictEqual(step.nav.pos, 2);
  assert.strictEqual(step.nav.draft, "");
  // Walk older.
  step = PromptNav.up(H, step.nav, "");
  assert.strictEqual(step.text, "second question");
  step = PromptNav.up(H, step.nav, "");
  assert.strictEqual(step.text, "first question");
  // Clamped at the oldest entry.
  step = PromptNav.up(H, step.nav, "");
  assert.strictEqual(step.text, "first question");
  assert.strictEqual(step.nav.pos, 0);
  // Walk back newer.
  step = PromptNav.down(H, step.nav);
  assert.strictEqual(step.text, "second question");
  step = PromptNav.down(H, step.nav);
  assert.strictEqual(step.text, "third question");
  // One past the newest restores the draft and disengages.
  step = PromptNav.down(H, step.nav);
  assert.strictEqual(step.text, "");
  assert.strictEqual(step.nav, null);
}

// The draft captured at engagement survives the round-trip.
{
  let step = PromptNav.up(H, null, "typed but unsent"); // newest, draft kept
  step = PromptNav.up(H, step.nav, ""); // older; draft must not be clobbered
  let newer = PromptNav.down(H, step.nav);
  assert.strictEqual(newer.text, "third question");
  assert.strictEqual(newer.nav.draft, "typed but unsent");
  const end = PromptNav.down(H, newer.nav);
  assert.strictEqual(end.text, "typed but unsent");
  assert.strictEqual(end.nav, null);
}

// Empty history: nothing to recall, navigation never engages.
{
  const step = PromptNav.up([], null, "");
  assert.strictEqual(step.text, null);
  assert.strictEqual(step.nav, null);
  const down = PromptNav.down([], null);
  assert.strictEqual(down.text, null);
  assert.strictEqual(down.nav, null);
}

// The list is re-read per step: a mid-navigation delete clamps instead of
// stranding the cursor on a stale index.
{
  let step = PromptNav.up(H, null, ""); // pos 2
  step = PromptNav.up(H, step.nav, ""); // pos 1
  const shrunk = ["first question"];
  step = PromptNav.up(shrunk, step.nav, "");
  assert.strictEqual(step.text, "first question");
  assert.strictEqual(step.nav.pos, 0);
  const down = PromptNav.down(shrunk, step.nav);
  assert.strictEqual(down.text, ""); // past the new end → draft back
  assert.strictEqual(down.nav, null);
}

console.log("prompt-nav: all assertions passed");
