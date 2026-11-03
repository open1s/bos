#!/usr/bin/env bash
# Render the GitHub release body for one version.
#
# The changelog entry is the human part; the rest is the install matrix and the
# limitations a downloader needs before they run the binary. Written to stdout
# so the workflow can hand it to action-gh-release as body_path.
#
# Usage: release-notes.sh 3.0.3 > notes.md

set -euo pipefail

version="${1:?usage: release-notes.sh <version>}"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
deb_version="$(printf '%s' "$version" | sed 's/-/~/')"

section="$(
  awk -v tag="v$version" '
    {
      # A literal prefix test, not a regex: a version contains dots, and
      # matching them as wildcards could pick the wrong heading.
      prefix = "## " tag
      if (substr($0, 1, length(prefix)) == prefix) {
        tail = substr($0, length(prefix) + 1, 1)
        if (tail == "" || tail == " ") { found = 1; next }
      }
      if (found && /^## /) { exit }
      if (found) { print }
    }
  ' "$repo_root/CHANGELOG.md"
)"

if [[ -z "${section//[[:space:]]/}" ]]; then
  section="_No changelog entry for this version._"
fi
# Drop the blank line between the changelog heading and its first bullet.
section="$(printf '%s\n' "$section" | sed '/./,$!d')"

# The install matrix is a literal template: artifact names are backticked
# Markdown, and an unquoted heredoc would run them as commands. @VERSION@ and
# @DEB@ are substituted after the block is emitted.
{
  printf '%s\n\n' "$section"
  cat <<'TEMPLATE'
## Install

| Platform | Artifact | How |
|---|---|---|
| macOS (Apple silicon) | `bsh-@VERSION@-macos-arm64.dmg` | Open the dmg, drag **BSH** to Applications. |
| macOS (Intel) | `bsh-@VERSION@-macos-x86_64.dmg` | Same as above. |
| Linux (x86_64) | `bsh_@DEB@_amd64.deb` / `bsh-@VERSION@-linux-x86_64.rpm` | `sudo apt install ./bsh_@DEB@_amd64.deb` or `sudo dnf install ./bsh-@VERSION@-linux-x86_64.rpm` |
| Linux (arm64) | `bsh_@DEB@_arm64.deb` / `bsh-@VERSION@-linux-aarch64.rpm` | Same, on an arm64 machine. |
| Linux (portable) | `bsh-@VERSION@-linux-<arch>.tar.gz` | `tar xzf ...` then `./install.sh` (installs to `~/.local`; `PREFIX` overrides). |
| Windows (x64) | `bsh-@VERSION@-windows-x64.msi` | Per-user install, no administrator rights required. |
| Windows (portable) | `bsh-@VERSION@-windows-x64.zip` | Unzip and run `bsh.exe`. |

The `.app.zip` files are the same signed bundles as the dmgs, for tools that
prefer a zip.

## Verification

`SHA256SUMS` is attached next to the artifacts:

```bash
sha256sum --check SHA256SUMS
```

## Platform notes

- **macOS** requires 11.0 or newer. Builds are signed when the Apple signing
  identity is configured, and notarized when the notary secrets are too;
  without them the app is ad-hoc signed, and Gatekeeper will refuse the first
  launch. In that case use
  **System Settings > Privacy & Security > Open Anyway**, or clear the
  quarantine attribute:
  `xattr -dr com.apple.quarantine /Applications/BSH.app`.
- **Linux** x86_64 builds need glibc 2.35 or newer (Ubuntu 22.04+, Debian 12+);
  arm64 builds need glibc 2.39 (Ubuntu 24.04+, Debian 13+). The WebView pane
  needs WebKitGTK 4.1, which the `.deb` declares and the `.rpm` discovers by
  soname. The RPM is unsigned. A Wayland or X11 session is required.
- **Windows** builds are x64 and unsigned unless the Windows signing secret is
  configured. The terminal pane is not available on Windows yet: the vendored
  Ghostty surface only implements macOS and Wayland, so the pane reports that
  it is unavailable. The browser pane needs the Edge WebView2 runtime, which
  ships with Windows 10 and 11. If the app does not start, install the
  Microsoft Visual C++ Redistributable (the MSVC runtime is linked dynamically).

Release automation and signing setup:
https://github.com/open1s/bos/blob/main/docs/bsh-release.md
TEMPLATE
} | sed -e "s/@VERSION@/$version/g" -e "s/@DEB@/$deb_version/g"
