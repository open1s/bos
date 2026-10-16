#!/usr/bin/env bash
# Assemble target/debug/BSH.app around the dev binary.
#
# A bare target/debug/bsh has no bundle identity, so macOS gives it a generic
# dock icon and no application name. This wraps the same binary in the smallest
# real bundle: the icon from src/assets, a name, and an identifier.
#
# The .ico next to the .icns is the Windows counterpart, read by Windows
# packaging rather than by this script.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
app="$root/target/debug/BSH.app"
icns="$root/crates/bsh/src/assets/bos-harness-icon.icns"

cargo build -p bsh

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$root/target/debug/bsh" "$app/Contents/MacOS/bsh"
cp "$icns" "$app/Contents/Resources/bos-harness-icon.icns"

cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>BSH</string>
  <key>CFBundleDisplayName</key><string>BOS Harness</string>
  <key>CFBundleExecutable</key><string>bsh</string>
  <key>CFBundleIdentifier</key><string>com.bos.bsh</string>
  <key>CFBundleIconFile</key><string>bos-harness-icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>3.0.2</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

# Refresh the icon cache so the change shows up without a logout.
touch "$app"
echo "built $app"
