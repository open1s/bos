#!/usr/bin/env bash
# Packaging guard for the install artifact (M14).
#
# Why a static guard: the bundle is assembled by `cargo tauri build`, which
# needs a network-plus-toolchain host and cannot run here. What *can* be checked
# is that the config describes an artifact that exists and that the icons are
# real files in the right formats — the failure this catches is the silent one,
# where a renamed or truncated icon still yields a bundle that simply has no
# icon, or a `frontendDist` that no longer points at the shipped UI.
set -u
cd "$(dirname "$0")/.."
CONF=tauri.conf.json
checks=0
fail=0
ok()   { checks=$((checks + 1)); }
bad()  { checks=$((checks + 1)); fail=$((fail + 1)); echo "  FAIL: $1"; }

[ -f "$CONF" ] && ok || bad "the bundler config is missing ($CONF)"

# --- the config must parse, and name the app the way the window does -------
# bash 3.2 (macOS) has no `mapfile` and chokes on an empty array under `set -u`,
# so the config's values travel as newline-separated text, not as arrays.
identifier=- product=- title=- frontend_dist=- global_tauri=- bundle_active=-
before_build=- version=-
cfg=$(python3 - "$CONF" <<'PY'
import json, sys
c = json.loads(open(sys.argv[1], encoding="utf-8").read())
app = c.get("app", {})
window = (app.get("windows") or [{}])[0]
build = c.get("build", {})
print("\n".join([
    c.get("identifier") or "-",
    c.get("productName") or "-",
    window.get("title") or "-",
    build.get("frontendDist") or "-",
    str(bool(app.get("withGlobalTauri"))).lower(),
    str(bool(c.get("bundle", {}).get("active"))).lower(),
    build.get("beforeBuildCommand") or "-",
    c.get("version") or c.get("bundle", {}).get("version") or "-",
]))
PY
)
[ -n "$cfg" ] || bad "$CONF is not valid JSON"
# One value per line, read as a whole line: a space in a product name or a build
# command must not shift every field after it (it did, before this was fixed).
cfg_line() { printf '%s\n' "$cfg" | sed -n "$1p"; }
identifier=$(cfg_line 1)
product=$(cfg_line 2)
title=$(cfg_line 3)
frontend_dist=$(cfg_line 4)
global_tauri=$(cfg_line 5)
bundle_active=$(cfg_line 6)
before_build=$(cfg_line 7)
version=$(cfg_line 8)

[ "$identifier" != "-" ] && ok || bad "the bundle has no identifier"
case "$identifier" in
  com.tauri.dev|com.tauri.*|"-") bad "identifier is still the Tauri placeholder ($identifier)" ;;
  *.*.*|*.*) case "$identifier" in
      *[!a-zA-Z0-9.-]*) bad "identifier is not reverse-DNS ($identifier)" ;;
      *) ok ;;
    esac ;;
  *) bad "identifier is not reverse-DNS ($identifier)" ;;
esac
[ "$product" != "-" ] && [ "$title" = "$product" ] \
  && ok || bad "the window title and productName must agree (product=$product title=$title)"

# --- the UI the bundle ships must be the one in the tree ------------------
[ -d "$frontend_dist" ] && ok || bad "frontendDist is not a directory ($frontend_dist)"
[ -s "$frontend_dist/index.html" ] && ok || bad "frontendDist has no index.html ($frontend_dist)"
# The webview reads window.__TAURI__, so withGlobalTauri is load-bearing: if the
# UI depends on it and the flag is off, every invoke is undefined at runtime.
tauri_used=$(grep -rl "__TAURI__" ui/*.js 2>/dev/null | wc -l | tr -d ' ')
if [ "$tauri_used" != "0" ]; then
  [ "$global_tauri" = "true" ] && ok || bad "ui/*.js reads __TAURI__ but withGlobalTauri is off"
else
  bad "__TAURI__ is not read anywhere in ui/*.js; the flag cannot be verified"
fi

# --- no build step between the tree and the artifact ----------------------
# `frontendDist` is committed source, so the artifact must not need a
# toolchain, a network, or a package manager to be assembled.
[ "$before_build" = "-" ] \
  && ok || bad "a beforeBuildCommand appeared ($before_build): re-verify a clean, offline bundle"

# --- the icons must exist and be the formats the platforms read -----------
[ "$bundle_active" = "true" ] && ok || bad "bundle.active is off: nothing would be produced"
icons=$(python3 - "$CONF" <<'PY'
import json, sys
c = json.loads(open(sys.argv[1], encoding="utf-8").read())
print("\n".join(c.get("bundle", {}).get("icon") or []))
PY
)
[ -n "$icons" ] && ok || bad "bundle.icon lists no icons"

missing=""
while IFS= read -r icon; do
  [ -n "$icon" ] || continue
  [ -s "$icon" ] || missing="$missing $icon"
done <<EOF
$icons
EOF
[ -z "$missing" ] && ok || bad "listed icons are missing or empty:$missing"

magic_bad=""
while IFS= read -r icon; do
  [ -s "$icon" ] || continue
  case "$icon" in
    *.png)  head -c 4 "$icon" | od -An -tx1 | tr -d ' \n' | grep -q '^89504e47' || magic_bad="$magic_bad $icon" ;;
    *.ico)  head -c 4 "$icon" | od -An -tx1 | tr -d ' \n' | grep -q '^00000100' || magic_bad="$magic_bad $icon" ;;
    *.icns) head -c 4 "$icon" | grep -q '^icns' || magic_bad="$magic_bad $icon" ;;
    *.svg)  grep -q '<svg' "$icon" || magic_bad="$magic_bad $icon" ;;
    *)      magic_bad="$magic_bad $icon(unchecked-format)" ;;
  esac
done <<EOF
$icons
EOF
[ -z "$magic_bad" ] && ok || bad "icon files whose bytes are not the format they claim:$magic_bad"

# Every desktop platform the bundler can target needs its own format.
for ext in png ico icns; do
  printf '%s\n' "$icons" | grep -q "\.$ext\$" \
    && ok || bad "no .$ext icon: the bundle would ship without one for that platform"
done

# --- version, when declared, must be a version ---------------------------
if [ "$version" = "-" ]; then
  ok   # Tauri derives it from Cargo.toml; not declaring it is fine.
else
  case "$version" in
    *[!0-9.]*|.*|*.) bad "version is not numeric-dotted ($version)" ;;
    *.*) ok ;;
    *) bad "version needs at least major.minor ($version)" ;;
  esac
fi

# --- the window must be allowed to listen, which is the one core surface it uses ---
# Tauri v2 answered every listen() with "core:event:allow-listen is not allowed"
# while this entry was missing. The app's own #[tauri::command]s are exempt from
# capabilities, so everything a fixture could see — rendering, invokes, the whole
# page guard — worked, and nothing at all arrived from the backend: no streamed
# token, no approval, no PTY output, no job or reminder update. A denial like that
# only exists in a running window, which is why it is checked here, in the file
# the bundle is built from.
perm=$(python3 - "$PWD" <<'PY'
import json, os, sys
root = sys.argv[1]
perms = []
conf = json.loads(open(os.path.join(root, "tauri.conf.json"), encoding="utf-8").read())
for entry in (conf.get("app", {}).get("security", {}).get("capabilities") or []):
    if isinstance(entry, str):
        perms.append(entry)
    else:
        perms.extend(entry.get("permissions") or [])
capdir = os.path.join(root, "capabilities")
if os.path.isdir(capdir):
    for name in sorted(os.listdir(capdir)):
        if name.endswith(".json"):
            cap = json.loads(open(os.path.join(capdir, name), encoding="utf-8").read())
            perms.extend(cap.get("permissions") or [])
print("\n".join(perms))
PY
)
printf '%s\n' "$perm" | grep -Eq 'core:(default|event:(default|allow-listen))' \
  && ok || bad "no capability grants the event channel: every listen() is refused at runtime"

# H0: the window has to overlay its title bar, or the webview is pushed down by the
# native strip and every y-target in the layout spec is off by its height. This is a
# native key, so no fixture can see it: the config is where it can be checked, and a
# key silently dropped here is a layout regression nothing else would catch.
chrome="$(python3 - "$PWD" <<'PY'
import json, os, sys
conf = json.loads(open(os.path.join(sys.argv[1], "tauri.conf.json"), encoding="utf-8").read())
win = (conf.get("app", {}).get("windows") or [{}])[0]
print(f'{win.get("titleBarStyle")}|{win.get("hiddenTitle")}')
PY
)"
case "$chrome" in
  "Overlay|True") ok ;;
  *) bad "H0: the window does not overlay its title bar (titleBarStyle|hiddenTitle=$chrome)" ;;
esac

echo "[package] checks=$checks failures=$fail"
[ "$fail" = 0 ]
