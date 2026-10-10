#!/usr/bin/env bash
# Assemble a minimal dev .app bundle around the built bsh binary.
#
# A bare binary run from target/debug gets a second-class dock icon on macOS:
# the dock sizes runtime-set icons with its own conform and the result reads
# larger than bundled apps no matter how the artwork is laid out. A real
# bundle (Info.plist + icns) goes through the exact same icon path as
# Docker/Chrome, so the dock icon matches by construction.
#
# Usage: crates/gui/make-dev-app.sh    (run after `cargo build -p bsh`)
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="$HERE/../../target/debug/bsh"
APP="$HERE/../../target/debug/BOS.app"
[ -x "$BIN" ] || { echo "bsh not built — run: cargo build -p bsh" >&2; exit 1; }

mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/bsh"
cp "$HERE/icons/bos-harness-icon.icns" "$APP/Contents/Resources/bos-harness-icon.icns"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>              <string>BOS</string>
    <key>CFBundleDisplayName</key>       <string>BOS</string>
    <key>CFBundleIdentifier</key>        <string>dev.bos.gui</string>
    <key>CFBundleVersion</key>           <string>3.0.2</string>
    <key>CFBundleShortVersionString</key><string>3.0.2</string>
    <key>CFBundlePackageType</key>       <string>APPL</string>
    <key>CFBundleExecutable</key>        <string>bsh</string>
    <key>CFBundleIconFile</key>          <string>bos-harness-icon</string>
    <key>LSMinimumSystemVersion</key>    <string>13.0</string>
    <key>NSHighResolutionCapable</key>   <true/>
</dict>
</plist>
PLIST
# Ad-hoc signature keeps Gatekeeper quiet for a locally built bundle.
codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || true
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$APP"
echo "dev bundle ready: $APP"
