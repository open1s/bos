#!/usr/bin/env bash
# Structural guard for the shipped markup and the accessibility wiring in app.js.
#
# Why a text guard: the transcript harness builds its own fixtures and never
# loads index.html, so the attributes the browser needs cannot be reached from
# it. These invariants are structural, so they are checked structurally here —
# and this is the only check that would catch a listbox that does not own its
# options, which is a bug that was actually shipped.
set -u
cd "$(dirname "$0")/.."
HTML=ui/index.html
JS=ui/app.js
checks=0
fail=0
ok()   { checks=$((checks + 1)); }
bad()  { checks=$((checks + 1)); fail=$((fail + 1)); echo "  FAIL: $1"; }

count() { grep -c -- "$1" "$2" 2>/dev/null || true; }

# Each listbox must be the element that holds the options, not a wrapper.
# Attribute order is not the invariant, so match the tag rather than a
# remembered string: a literal pattern here once passed on a stale shape and
# failed on the correct one.
tag_role() { grep -qE "<$1 id=\"$2\"[^>]*role=\"$3\"" $HTML; }
[ "$(grep -cE '<ul id="cmd-list"[^>]*role="listbox"' $HTML)" = 1 ] \
  && ok || bad "the command list must own role=listbox"
[ "$(grep -cE '<ul id="mention-list"[^>]*role="listbox"' $HTML)" = 1 ] \
  && ok || bad "the mention list must own role=listbox"
[ "$(grep -cE '<div id="cmd-palette"[^>]*role=' $HTML)" = 0 ] \
  && ok || bad "the palette wrapper must not claim to be the listbox"
[ "$(grep -cE '<div id="mention-pop"[^>]*role=' $HTML)" = 0 ] \
  && ok || bad "the mention wrapper must not claim to be the listbox"

# The input that drives the palette must say so.
grep -q 'id="input" rows="1" aria-controls="cmd-list" aria-expanded="false"' $HTML \
  && ok || bad "the composer must control the palette and report its state"

# Every modal must be a labelled dialog, and each label must exist once.
for id in settings approval; do
  grep -q "id=\"$id-modal\" class=\"modal hidden\" role=\"dialog\"" $HTML \
    && ok || bad "#$id-modal must be role=dialog"
  grep -q "id=\"$id-modal\"" $HTML && grep -A1 "id=\"$id-modal\"" $HTML | grep -q 'aria-modal="true"' \
    && ok || bad "#$id-modal must be aria-modal"
  grep -A1 "id=\"$id-modal\"" $HTML | grep -q "aria-labelledby=\"$id-title\"" \
    && ok || bad "#$id-modal must be labelled"
  [ "$(count "id=\"$id-title\"" $HTML)" = 1 ] \
    && ok || bad "the #$id-modal label must exist exactly once"
done

# The option rows must carry their state, and the input must point at the
# selected one.
grep -q 'li.id = `cmd-opt-${i}`;' $JS && ok || bad "options need stable ids"
grep -q 'li.setAttribute("aria-selected", i === p.index ? "true" : "false");' $JS \
  && ok || bad "options must report which one is selected"
grep -q 'input.setAttribute("aria-activedescendant", `cmd-opt-${p.index}`);' $JS \
  && ok || bad "the input must name the active option"
grep -q 'input.removeAttribute("aria-activedescendant");' $JS \
  && ok || bad "closing the palette must clear the active option"
grep -q 'input.setAttribute("aria-expanded", "false");' $JS \
  && ok || bad "closing the palette must report it closed"

# Focus has to go somewhere deliberate in both directions.
grep -q 'state.settingsOpener = document.activeElement;' $JS \
  && ok || bad "opening settings must remember the opener"
grep -q 'if (back && typeof back.focus === "function" && document.contains(back)) back.focus();' $JS \
  && ok || bad "closing settings must hand focus back"
grep -q 'const allow = $("approval-allow");' $JS && ok || bad "the approval dialog must take focus"
grep -q 'approval-allow' $HTML && ok || bad "the approval dialog needs a focusable choice"

# Icon-only controls need a name that is not a glyph.
for label in "Rename workspace" "Remove workspace" "Move this chat to the trash"; do
  grep -q "setAttribute(\"aria-label\", \"$label\")" $JS \
    && ok || bad "icon-only button needs aria-label: $label"
done

# The header's view switch must be a real tab strip: a tablist owns its two
# tabs, each tab names a pane that exists, the panes label their tabs back,
# and only one tab claims selection at load.
[ "$(grep -cE '<div id="center-tabs"[^>]*role="tablist"' $HTML)" = 1 ] \
  && ok || bad "the center strip must own role=tablist"
[ "$(grep -cE 'role="tab"' $HTML)" = 2 ] \
  && ok || bad "exactly two tabs must exist in the strip"
for tab in tab-chat:messages tab-trajectory:trajectory; do
  t="${tab%%:*}"; p="${tab##*:}"
  grep -qE "<button[^>]*id=\"$t\"[^>]*role=\"tab\"" $HTML \
    && ok || bad "#$t must be role=tab"
  grep -qE "<button[^>]*id=\"$t\"[^>]*aria-controls=\"$p\"" $HTML \
    && ok || bad "#$t must control its pane #$p"
  grep -qE "<(div|section) id=\"$p\"[^>]*role=\"tabpanel\"" $HTML \
    && ok || bad "#$p must be role=tabpanel"
  grep -qE "<(div|section) id=\"$p\"[^>]*aria-labelledby=\"$t\"" $HTML \
    && ok || bad "#$p must label its tab #$t back"
done
grep -qE '<button[^>]*id="tab-chat"[^>]*aria-selected="true"[^>]*tabindex="0"' $HTML \
  && ok || bad "Chat must load selected and own the strip's tab stop"
grep -qE '<button[^>]*id="tab-trajectory"[^>]*aria-selected="false"[^>]*tabindex="-1"' $HTML \
  && ok || bad "Trajectory must load unselected, out of the tab order"

echo "[markup] checks=$checks failures=$fail"
[ "$fail" = 0 ]
