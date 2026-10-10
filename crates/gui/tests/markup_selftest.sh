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
CSSFILE=ui/style.css
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

# Icon-only controls ante their names where the row is built. The invariant that
# matters — every glyph-only control on the page has an accessible name — is
# asserted in page_checks.js against the rendered DOM, where the rows, the pin
# and an *open* menu can all be seen; here it is only the wiring that the text
# can still see. A static list of the old glyphs' labels was green while the
# controls it named no longer existed, which is why it is a rendered check now.
for label in 'Actions for ' 'New session in '; do
  grep -q "setAttribute(\"aria-label\", .*$label" $JS \
    && ok || bad "icon-only button needs a named row: $label"
done
grep -q 'pin.setAttribute("aria-label"' $JS && ok || bad "the pin needs a name"
grep -q 'pin.setAttribute("aria-pressed"' $JS && ok || bad "the pin must report its state"
# The menu itself is labelled by the button that opened it, and says so.
grep -q 'menu.setAttribute("aria-label"' $JS && ok || bad "the row menu needs a name"
grep -q 'menu.setAttribute("role", "menu")' $JS && ok || bad "the row menu must be a menu"
[ "$(grep -c 'setAttribute("role", "menuitem")' $JS)" -ge 1 ] \
  && ok || bad "the row menu's entries must be menu items"
# The workspace rename/remove forms still exist and are still reachable — from
# the menu now. A guard that only checked the glyphs would have let the forms go.
for run in '"rename"' '"delete"'; do
  grep -q "item.run ===" $JS && ok || bad "the workspace menu must dispatch"
done
grep -q 'openWorkspaceForm("edit", group.id)' $JS && ok || bad "rename a workspace must stay reachable"
grep -q 'openWorkspaceForm("remove", group.id)' $JS && ok || bad "removing a workspace must stay reachable"

# The header's view switch must be a real tab strip: a tablist owns its two
# tabs, each tab names a pane that exists, the panes label their tabs back,
# and only one tab claims selection at load.
[ "$(grep -cE '<div id="center-tabs"[^>]*role="tablist"' $HTML)" = 1 ] \
  && ok || bad "the center strip must own role=tablist"
# Scoped to the strip: the settings rail is a second tablist in the same file,
# so a document-wide count would (and did) report a false failure.
center_tabs=$(python3 - "$PWD/$HTML" <<'PYEOF'
import re, sys
html = open(sys.argv[1], encoding="utf-8").read()
start = html.index('<div id="center-tabs"')
i = start
depth = 0
while True:
    nxt = re.search(r"<(/?)div\b[^>]*>", html[i:])
    if not nxt:
        break
    depth += -1 if nxt.group(1) else 1
    i += nxt.end()
    if depth == 0:
        break
strip = html[start:i]
print(len(re.findall(r'role="tab"', strip)))
PYEOF
)
[ "$center_tabs" = 2 ] \
  && ok || bad "exactly two tabs must exist in the strip (found ${center_tabs:-none})"
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


# --- M13 polish: the chrome the polish layer drives -------------------------
# The skip link must be the body's first element (it is only useful before
# anything else takes focus) and must point at the conversation.
grep -q '<body>$\|^  <a id="skip-link"' $HTML && ok || bad "the body must open normally"
[ "$(grep -c 'id="skip-link" class="skip-link" href="#messages"' $HTML)" = 1 ] \
  && ok || bad "the skip link must target #messages"
SKIP_LINE=$(grep -n 'id="skip-link"' $HTML | cut -d: -f1)
DIV_LINE=$(grep -n 'id="app"' $HTML | head -1 | cut -d: -f1)
[ -n "$SKIP_LINE" ] && [ -n "$DIV_LINE" ] && [ "$SKIP_LINE" -lt "$DIV_LINE" ] \
  && ok || bad "the skip link must precede the application shell"

# The two header preferences and the announced stream state.
for id in theme-toggle lang-toggle; do
  grep -qE "<button[^>]*id=\"$id\"" $HTML && ok || bad "#$id must exist in the header"
done
grep -qE '<span id="stream-chip"[^>]*aria-live="polite"' $HTML \
  && ok || bad "the stream chip must be a polite live region"
grep -qE '<textarea id="input"[^>]*aria-keyshortcuts="[^"]*Enter' $HTML \
  && ok || bad "the composer must announce its send keys"

# Scripts: the polish layer loads last, and only after the shell it reads from.
grep -q '<script src="polish.js"></script>' $HTML && ok || bad "polish.js must be loaded"
[ "$(grep -c '<script src=' $HTML)" = 4 ] && ok || bad "exactly four scripts must load"
SHELL_LINE=$(grep -n '<script src="shell.js"' $HTML | cut -d: -f1)
POLISH_LINE=$(grep -n '<script src="polish.js"' $HTML | cut -d: -f1)
[ -n "$SHELL_LINE" ] && [ "$SHELL_LINE" -lt "$POLISH_LINE" ] \
  && ok || bad "polish.js must load after shell.js"

# Every translation mark must have a string, or the element silently keeps
# whatever it shipped with and a typo'd key is invisible.
missing=0
for key in $(grep -oE 'data-i18n(-placeholder|-aria|-title)?="[^"]+"' $HTML \
               | sed 's/.*="//; s/"//' | sort -u); do
  grep -q "\"$key\":" ui/polish.js || { missing=$((missing + 1)); echo "  missing string: $key"; }
done
[ "$missing" = 0 ] && ok || bad "$missing translation mark(s) have no string in polish.js"

# No id may appear twice: a duplicate was actually shipped (the static drawer
# button and the shell's injected rail button both claimed sidebar-toggle).
dupes=$(grep -oE ' id="[^"]+"' $HTML | sed 's/.*="//; s/"//' | sort | uniq -d | tr '\n' ' ')
[ -z "$dupes" ] && ok || bad "duplicate ids in the shipped markup: $dupes"

# Every custom property the stylesheet *uses* must be one it *defines*. Five
# were not (`--muted`, `--fg`, `--panel`, `--line`, `--input-bg`, 27 uses):
# each carried a dark-only hard-coded fallback, so light mode rendered
# near-white text on white and no guard noticed. A used-but-undefined token is
# always a mistake, so it is checked as one.
undefined_tokens=$(python3 - "$PWD/$CSSFILE" <<'PYEOF'
import re, sys
css = open(sys.argv[1], encoding="utf-8").read()
defined = set(re.findall(r"(--[a-z0-9-]+)\s*:", css))
used = set(re.findall(r"var\((--[a-z0-9-]+)", css))
print(" ".join(sorted(used - defined)))
PYEOF
)
[ -z "$undefined_tokens" ] && ok || bad "style.css uses undefined custom properties: $undefined_tokens"

# A translation mark must wrap text, never elements: applying a translation
# replaces the mark's children, so a mark on a wrapper silently deletes the
# icons inside it (that is exactly how four rail icons disappeared).
i18n_nested=$(python3 - "$PWD/$HTML" <<'PYEOF'
import re, sys
html = open(sys.argv[1], encoding="utf-8").read()
bad = []
for m in re.finditer(r'<([a-zA-Z][a-zA-Z0-9]*)([^>]*?)\sdata-i18n="([^"]+)"([^>]*)>', html):
    tag, key = m.group(1), m.group(3)
    close = html.find(f"</{tag}>", m.end())
    inner = html[m.end():close if close != -1 else len(html)]
    if "<" in inner:
        bad.append(key)
print(" ".join(sorted(set(bad))))
PYEOF
)
[ -z "$i18n_nested" ] && ok || bad "data-i18n marks that would delete their children: $i18n_nested"

# The icons ship twice on purpose — `crates/gui/icons/` is the source of truth
# the user maintains, `ui/icons/` is what the app can actually serve (the
# frontend dist is `ui`, so `../icons` is unreachable). A copy that silently
# drifts is worse than no copy, so the served SVG must equal the source byte for
# byte.
drifted=""
for svg in icons/*.svg; do
  [ -e "$svg" ] || continue
  base=$(basename "$svg")
  cmp -s "$svg" "ui/icons/$base" || drifted="$drifted $base"
done
[ -z "$drifted" ] && ok || bad "ui/icons/ differs from the source icons:$drifted (re-copy crates/gui/icons/*.svg)"

# `$` is getElementById, so a '#' in the argument makes the lookup return null
# and the caller's `if (!el) return` turns a whole feature into a silent no-op.
# Three of these lived in the plan renderer — which is why the shipped todo list
# never drew and its count never filled, while every guard stayed green.
dollar_hash=$(python3 - "$PWD/ui" <<'PYEOF'
import pathlib, re, sys
bad = []
for path in sorted(pathlib.Path(sys.argv[1]).glob("*.js")):
    for n, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if re.search(r"\$\(\s*['\"]#", line):
            bad.append(f"{path.name}:{n}")
print(" ".join(bad))
PYEOF
)
[ -z "$dollar_hash" ] && ok || bad "a '#' selector passed to \$ (getElementById, not querySelector): $dollar_hash"

echo "[markup] checks=$checks failures=$fail"
[ "$fail" = 0 ]
