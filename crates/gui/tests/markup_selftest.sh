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
[ "$(grep -c 'setAttribute("role", item.checked === undefined ? "menuitem" : "menuitemradio")' $JS)" -ge 1 ] \
  && ok || bad "the row menu's entries must be menu items, and a choice must be a radio"
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

# --- the sidebar is the conversation list (rev 110) -------------------------
# Four utility rows used to sit above it (Plugins & skills, Automation tasks,
# MCP connectors, Files). Three duplicated a Settings section that already
# existed, and the user asked for them gone; Files moved to the rail beside its
# siblings. The rows are deleted rather than hidden, so a test *claiming* they
# are gone has to read the shipped markup — no fixture can see index.html.
[ "$(grep -cE '<nav class="sidebar-nav"' $HTML)" = 0 ] \
  && ok || bad "the sidebar's utility nav must be gone from the markup"
[ "$(grep -cE 'class="nav-item"' $HTML)" = 0 ] \
  && ok || bad "no nav-item rows may remain in the shipped markup"
[ "$(grep -cE 'id="files-toggle"|class="files-head"' $HTML)" = 0 ] \
  && ok || bad "the sidebar Files toggle must be gone"
[ "$(grep -cE 'sidebar-nav|nav-item|files-head' $CSSFILE)" = 0 ] \
  && ok || bad "the stylesheet must not keep rules for the removed nav rows"
# Files is now a Settings section: tab and pane must exist, point at each other,
# and the tree must live *inside* the pane rather than under the sidebar.
grep -qE '<button[^>]*role="tab"[^>]*id="set-tab-files-section"[^>]*aria-controls="files-section"' $HTML \
  && ok || bad "the rail must carry a Files tab that controls #files-section"
# A pane's opening tag wraps in this file (id and role on one line, the label on
# the next), so the two halves are matched separately rather than as one tag.
grep -qE '<(div|section) id="files-section"[^>]*role="tabpanel"' $HTML \
  && grep -qE 'aria-labelledby="set-tab-files-section"' $HTML \
  && ok || bad "#files-section must be a tabpanel labelled by its tab"
FILE_TREE_LINE=$(grep -n 'id="file-tree"' $HTML | cut -d: -f1)
SIDE_WS_LINE=$(grep -n 'id="side-workspaces"' $HTML | head -1 | cut -d: -f1)
[ -n "$FILE_TREE_LINE" ] && [ "$FILE_TREE_LINE" -gt "$SIDE_WS_LINE" ] \
  && ok || bad "the file tree must sit after the sidebar, inside the Files pane"
# And inside the pane itself: scoping the search to the section is what makes
# this an assertion about the pane rather than about the document.
FILES_PANE=$(sed -n '/<section id="files-section"/,/<\/section>/p' $HTML)
case "$FILES_PANE" in
  *'id="file-tree"'*) ok ;;
  *) bad "the tree host must be inside #files-section" ;;
esac
# The palette is the keyboard path to what the sidebar no longer lists.
grep -qE '"\/files", hint: "[^"]*", run: \(\) => navTo\("files-section"\)' $JS \
  && ok || bad "/files must open the Files section, not a removed sidebar toggle"

# --- the Workspaces header carries search, a view menu, and add (rev 111/112) -
# It was "New session", then a labelled button with a row of its own, then an
# icon in the brand row for one round; it is the folder-plus in the header of the
# section it adds to now, as the design shows, and the brand row is the brand
# alone again. Two identical folder-plus glyphs 40 px apart are a duplicate, not
# a shortcut, so the count of them is asserted too.
SIDEBAR_HEAD=$(sed -n '/<div class="sidebar-head">/,/<\/div>/p' $HTML)
case "$SIDEBAR_HEAD" in
  '') bad "the sidebar head row must exist" ;;
  *'workspace-add-primary'*) bad "the brand row holds the brand alone; the folder-plus moved to the Workspaces header" ;;
  *) ok ;;
esac
# Scoped to the section header's own action row: this is an assertion about that
# row rather than about the document. (The opening tags wrap, so the id and the
# class are matched as positions in the block, not as one tag.)
SIDE_ACTIONS=$(sed -n '/<div class="side-actions">/,/<\/div>/p' $HTML)
case "$SIDE_ACTIONS" in
  *'id="sessions-search"'*'id="session-view"'*'id="workspace-add-primary"'*) ok ;;
  *) bad "the Workspaces header must hold search, the view menu and add-workspace, in that order" ;;
esac
case "$SIDE_ACTIONS" in
  *'aria-label="Add a workspace"'*) ok ;;
  *) bad "the header's add-workspace control must carry its name" ;;
esac
grep -q 'data-i18n-aria="btn.addWorkspace"' $HTML && grep -q 'data-i18n-title="btn.addWorkspace"' $HTML \
  && ok || bad "that icon must carry its translated name and tooltip"
grep -q 'data-i18n="btn.addWorkspace"' $HTML \
  && bad "an icon-only control must not keep a visible label it no longer draws" || ok
# The sliders button announces a menu, not a toggle: it opens a list of choices.
case "$SIDE_ACTIONS" in
  *'id="session-view"'*'aria-haspopup="menu"'*) ok ;;
  *) bad "the view control must announce the menu it opens" ;;
esac
STALE_NAMES=$(grep -cE 'id="new-chat"|id="workspace-add"|class="new-session"|new-session-mark|side-primary|sessions-refresh|archived-toggle|showArchived' \
              $HTML $CSSFILE $JS | awk -F: '{n+=$NF} END {print n+0}')
[ "$STALE_NAMES" = 0 ] \
  && ok || bad "no stale new-session, duplicate workspace-add, refresh or archive-disclosure name may survive ($STALE_NAMES found)"
# --- the view menu is the one writer of how the list is drawn (rev 112) ------
# Three choices, each with its own vocabulary: a mode that exists in the state
# but not in the menu is a mode nobody can reach, and one in the menu but not in
# the state is a control that does nothing.
for choice in 'Workspace Tree' 'In one list' 'Last updated' 'Hide archived' 'All conversations (show archived)' 'Archived only'; do
  grep -qF "$choice" $JS && ok || bad "the view menu must offer $choice"
done
grep -qE 'const SESSION_VIEW_KEY = "bos\.sessionView"' $JS && grep -q 'persistSessionView();' $JS \
  && grep -q 'restoreSessionView();' $JS \
  && ok || bad "the view choice must be written, read back, and named by one key"
# The reload the header used to carry has to survive *whole* in the menu: an item
# that is offered and a handler that acts on it are two halves of one gesture, and
# either half alone is a control that does nothing.
grep -qF 'label: tr("view.reload", "Reload chats"), run: "reload"' $JS \
  && grep -qF 'if (key === "reload") return void reloadSessionList()' $JS \
  && ok || bad "the reload gesture the header used to carry must survive in the menu"
# A gesture removed from the chrome must keep a door: three, and each is
# asserted so "the button is gone" cannot quietly mean "a chat is unreachable".
grep -qE 'if \(key === "n"\) \{' $JS && grep -q 'void newChat();' $JS \
  && ok || bad "Cmd/Ctrl+N must still start a chat"
grep -qE '"\/new", hint: "Start a new chat", run: \(\) => newChat\(\)' $JS \
  && ok || bad "/new must still start a chat"
# The row's own "+" must start a session *in that row's workspace*: scoped to the
# listener that follows the button's construction, because the row's "…" menu
# offers the same action and would satisfy a document-wide grep on its own
# (measured: rewiring the "+" to newChat() left such a check green).
# The folder and the disclosure share one slot in a workspace row, which takes both
# ends: the stylesheet hides the folder only on a row the code marked foldable, so a
# row that cannot fold must not hover its folder away for a triangle it does not have.
grep -qF '.ws-head.foldable:hover .ws-folder { display: none; }' $CSSFILE \
  && grep -qF 'head.classList.add("foldable")' $JS \
  && ok || bad "the row's folder and its disclosure must trade one slot under the pointer"
WS_NEW_BLOCK=$(grep -A 9 'const add = el("button", "ws-act ws-new")' $JS)
case "$WS_NEW_BLOCK" in
  *'void newSessionIn(group.id);'*) ok ;;
  *) bad "a workspace row's + must still start a session in that workspace" ;;
esac
grep -qE 'add.setAttribute\("aria-keyshortcuts", "Meta\+N Control\+N"\)' $JS \
  && ok || bad "that row control must advertise the shortcut it answers to"


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
# whatever it shipped with and a typo'd key is invisible. The marks are the
# markup's `data-i18n*` attributes **and** the keys app.js asks `tr()` for: a
# label built in JS never appears in the markup scan, which is how seven keys —
# the Add-workspace dialog's hints among them — rendered English in a Chinese UI
# while this check stayed green.
missing=0
for key in $(grep -oE 'data-i18n(-placeholder|-aria|-title)?="[^"]+"' $HTML \
               | sed 's/.*="//; s/"//' | sort -u) \
           $(grep -oE 'tr\("[a-zA-Z0-9_.]+"' $JS | sed 's/^tr("//; s/"$//' | sort -u); do
  grep -q "\"$key\":" ui/polish.js || { missing=$((missing + 1)); echo "  missing string: $key"; }
done
[ "$missing" = 0 ] && ok || bad "$missing translation mark(s) have no string in polish.js"

# The Add-workspace dialog tells the user what creating a workspace does to the
# folder they are about to pick, and that sentence is a claim about code no guard
# on this page can see — the Rust side writes `.bos` into it. The claim it
# replaced ("Nothing is changed in the folder itself") was false from the day
# creation started writing there, and nothing caught it; so the claim that replaced
# it is checked as the sentence a reader actually gets, not as a key.
bos_claim=$(python3 - "$PWD/$HTML" <<'PYEOF'
import re, sys
html = open(sys.argv[1], encoding="utf-8").read()
m = re.search(r'<p[^>]*data-i18n="ws.add.bos"[^>]*>(.*?)</p>', html, re.S)
print("" if (m and ".bos" in m.group(1)) else "missing")
PYEOF
)
[ -z "$bos_claim" ] && ok || bad "the creation dialog must say the folder gets its own .bos configuration"

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

# Flat is a rule, so it is checked like one. The UI carries no shadow, gradient,
# blur or brightness filter — depth comes from a 1px border and a solid fill —
# and every corner comes from the radius scale, so the chrome cannot drift back
# into nine different radii. Two things are deliberately not violations and are
# named here so a third cannot be added quietly: the streaming caret's `steps(1)`
# blink (a text caret, one paint per second) and the mobile drawer's `transform`
# slide (a motion, not a surface). Flat is about surfaces, not about movement.
# Two radius values are shapes rather than corners and are allowed: `999px` for a
# pill and `50%` for a circle.
flat_offenders=$(python3 - "$PWD/$CSSFILE" <<'PYEOF'
import re, sys
lines = open(sys.argv[1], encoding="utf-8").read().splitlines()
bad = []
for n, line in enumerate(lines, 1):
    decl = line.split("/*")[0]
    if not decl.strip():
        continue
    for prop in ("box-shadow", "text-shadow", "backdrop-filter", "filter"):
        m = re.search(r"(?<![-\w])" + prop + r"\s*:\s*([^;]+);", decl)
        if m and m.group(1).strip() != "none":
            bad.append("%d:%s=%s" % (n, prop, m.group(1).strip()))
    if re.search(r"gradient\(", decl):
        bad.append("%d:gradient" % n)
    m = re.search(r"border-radius\s*:\s*([^;]+);", decl)
    if m:
        for part in m.group(1).split():
            if re.fullmatch(r"[\d.]+px", part) and part not in ("0px", "999px"):
                bad.append("%d:radius=%s" % (n, part))
    m = re.search(r"transition\s*:\s*([^;]+);", decl)
    if m and "transform" not in m.group(1):
        for value in re.findall(r"([\d.]+)(m?s)\b", m.group(1)):
            ms = float(value[0]) * (1 if value[1] == "ms" else 1000)
            if ms > 120:
                bad.append("%d:transition=%dms" % (n, int(ms)))
print(" ".join(bad))
PYEOF
)
[ -z "$flat_offenders" ] && ok || bad "style.css is not flat: $flat_offenders"

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
