/* Prompt-history navigation for the composer.
 *
 * The pure state machine behind pressing ↑/↓ in the chat input: it walks the
 * active session's stored user prompts and restores the original draft once
 * you scroll past the newest entry. Deliberately DOM-free so it can run both
 * in the WebView and under plain `node` for testing (see prompt-nav.test.js).
 *
 * The list is passed in on every step instead of being snapshotted, so edits,
 * deletes, and sends that happen mid-navigation can never strand the cursor
 * on a stale index — positions simply clamp to the current list.
 *
 * State shape: { pos, draft } | null (null = not navigating).
 * `pos == list.length` is the "virtual slot" past the newest entry.
 */
const PromptNav = {
  /* Move one entry older. `nav` may be null (navigation not yet engaged),
     in which case start from the newest entry and remember `draft` so ↓
     can bring it back. Returns { nav, text } where text === null means
     "nothing to recall" (empty history). */
  up(list, nav, draft) {
    if (!list.length) return { nav: null, text: null };
    const cur = nav ? Math.min(nav.pos, list.length) : list.length;
    const pos = Math.max(0, cur - 1);
    return { nav: { pos, draft: nav ? nav.draft : draft }, text: list[pos] };
  },

  /* Move one entry newer; passing the newest entry restores the draft and
     disengages. Returns { nav: null, text: null } if not navigating. */
  down(list, nav) {
    if (!nav) return { nav: null, text: null };
    const pos = nav.pos + 1;
    if (pos >= list.length) return { nav: null, text: nav.draft };
    return { nav: { pos, draft: nav.draft }, text: list[pos] };
  },
};

/* Browser global for app.js; CommonJS export for the node test. */
if (typeof window !== "undefined") window.PromptNav = PromptNav;
if (typeof module !== "undefined" && module.exports) module.exports = PromptNav;
