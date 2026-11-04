# Releasing BSH

BSH ships as native installers for macOS, Linux and Windows. They are built by
[`.github/workflows/release.yml`](../.github/workflows/release.yml) from a
version tag; nothing is assembled by hand.

## What ships

| Platform | Installer | Portable | Built on |
|---|---|---|---|
| macOS, Apple silicon | `bsh-<version>-macos-arm64.dmg` | `bsh-<version>-macos-arm64.app.zip` | `macos-15` |
| macOS, Intel | `bsh-<version>-macos-x86_64.dmg` | `bsh-<version>-macos-x86_64.app.zip` | `macos-15-intel` |
| Linux, x86_64 | `bsh_<deb-version>_amd64.deb`, `bsh-<version>-linux-x86_64.rpm` | `bsh-<version>-linux-x86_64.tar.gz` | `ubuntu-22.04` |
| Linux, arm64 (best-effort) | `bsh_<deb-version>_arm64.deb`, `bsh-<version>-linux-aarch64.rpm` | `bsh-<version>-linux-aarch64.tar.gz` | `ubuntu-24.04-arm` |
| Windows, x64 | `bsh-<version>-windows-x64.msi` | `bsh-<version>-windows-x64.zip` | `windows-2022` |

Every release also carries `SHA256SUMS`. The arm64 job is best-effort: GitHub's
free arm64 runners are periodically preempted mid-build, so a release publishes
arm64 when that runner survives and otherwise ships the x86_64 Linux packages
only. The arm64 job never blocks the release.

## The version contract

A release exists in two places, and they must agree:

1. the workspace version in `Cargo.toml` (`[workspace.package] version`),
2. the git tag, `v<version>`.

The `prepare` job reads the tag, compares it with the manifest, and fails before
any build starts if they differ. This is deliberate: the binary prints
`CARGO_PKG_VERSION` in About, so a package whose number disagrees with its tag
is a support problem. A manual dispatch may pass the expected version, and it is
checked the same way.

Version-format constraints worth knowing before choosing a number:

- **Debian** cannot express a hyphen inside a version, so `3.0.3-rc.1` becomes
  `3.0.3~rc.1` (which correctly sorts *before* `3.0.3`).
- **MSI** takes a numeric `major.minor.build` only, so a prerelease suffix is
  dropped from the package metadata; use a distinct patch number per
  prerelease.
- **RPM** also cannot carry a hyphen in `Version`, so the same suffix moves
  into `Release`: `3.0.3-rc.1` becomes version `3.0.3`, release `0.rc.1`.
- **macOS** `CFBundleShortVersionString` is numeric, so the suffix lives in the
  artifact name only.

Add the matching `## v<version> (date)` section to [`CHANGELOG.md`](../CHANGELOG.md);
the release notes are rendered from it.

## Cut a release

The repository uses jj, so the release commit is finished before it is tagged:

```bash
# 1. Bump [workspace.package] version and add the changelog entry.
jj describe -m "release: BSH 3.0.3"
jj new

# 2. Tag the release commit (the working copy's parent), then push code + tag.
jj tag set v3.0.3 -r @-
jj git push                    # the release commit onto main
jj git push --tag v3.0.3       # starts the release workflow
```

The tag push starts the workflow. It builds five platform jobs in parallel,
with arm64 best-effort, then a single `release` job downloads every artifact, checks every expected
artifact is present, writes `SHA256SUMS`, and creates the GitHub release with
notes generated from the changelog.

Tag a commit whose `rust` workflow is green. The pipeline's `verify` job
re-runs the workspace test suite on the tagged tree before anything is
published, and each platform job builds and smoke-tests its own binary, so a
tag cannot publish a tree that does not compile or test.

## Dry run

Run **Actions → release → Run workflow**, leave `version` empty (it reads the
manifest) and leave `publish` off. The artifacts are built and left on the run
for download; no release is created. Set `publish` to true only when you mean
it, and note that a version without a tag needs the same number in
`Cargo.toml`.

Local runs of the packagers use the same scripts the workflow calls:

```bash
# macOS (host architecture): produces a dmg and an app zip
bash scripts/ci/package-macos.sh --version 3.0.3 --binary target/release/bsh --out dist

# Linux: produces a .deb (when dpkg-deb exists) and a portable tarball
bash scripts/ci/package-linux.sh --version 3.0.3 --binary target/release/bsh --out dist

# Windows (PowerShell, from the repository root)
pwsh ./scripts/ci/package-windows.ps1 -Version 3.0.3 `
  -Binary target/release/bsh.exe -OutDir dist
```

Use `target/debug/bsh` to exercise the packaging without an LTO release build;
the bundle is identical, only the binary is bigger.

## Signing and notarization

Signing is optional and driven by repository secrets. Without the signing
identity the workflow still produces working packages: macOS falls back to an
ad-hoc signature and Windows stays unsigned. Notarization additionally needs
the three notary values; signing alone does not satisfy Gatekeeper.

| Secret | Format | Effect |
|---|---|---|
| `APPLE_CERTIFICATE_P12` | base64 of a `.p12` | Imports the Developer ID certificate into a temporary keychain. |
| `APPLE_CERTIFICATE_PASSWORD` | string | Password for that `.p12`. |
| `APPLE_SIGNING_IDENTITY` | `Developer ID Application: Name (TEAMID)` | Signs the app and dmg with the hardened runtime. |
| `APPLE_ID` | Apple ID email | Notarization. |
| `APPLE_TEAM_ID` | 10-character team ID | Notarization. |
| `APPLE_APP_PASSWORD` | app-specific password | Notarization. |
| `WINDOWS_CERTIFICATE_PFX` | base64 of a `.pfx` | Signs `bsh.exe` and the MSI. |
| `WINDOWS_CERTIFICATE_PASSWORD` | string | Password for that `.pfx`. |

Create the base64 blobs locally without committing them:

```bash
# macOS / Linux
base64 -i certificate.p12 | pbcopy          # macOS
base64 -w0 certificate.p12                  # Linux
```

```powershell
# Windows
[Convert]::ToBase64String([IO.File]::ReadAllBytes("certificate.pfx"))
```

When all three Apple notary values are present, the macOS job signs the app,
submits a zip for notarization, staples the ticket into `BSH.app`, and then
builds the dmg and zip from the stapled bundle. One submission covers both
artifacts. Signing uses the hardened runtime with
[`installer/bsh.entitlements`](../installer/bsh.entitlements); the
`com.apple.security.cs.allow-jit` entitlement is required because Wasmtime
compiles extension components at runtime.

A signed but unnotarized macOS app is still refused by Gatekeeper on first
launch; the build prints a warning when that is the case.

## How the pieces fit

| File | Responsibility |
|---|---|
| `.github/workflows/release.yml` | Tag trigger, five-platform matrix, artifact verification, GitHub release. |
| `scripts/ci/resolve-release-version.sh` | Tag/manifest agreement, SemVer validation, publish/prerelease flags. |
| `scripts/ci/package-macos.sh` | Staged `BSH.app`, optional signing/notarization/stapling, dmg, zip. |
| `scripts/ci/package-linux.sh` | `.deb` with derived `Depends`, `.rpm` with soname `Requires`, portable tarball and `install.sh`. |
| `scripts/ci/package-windows.ps1` | Portable zip and per-user MSI via WiX, optional Authenticode. |
| `scripts/ci/release-notes.sh` | Release body from the changelog plus the install matrix. |
| `installer/bsh.wxs` | WiX 3 source for the per-user MSI. |
| `installer/bsh.entitlements` | Hardened-runtime entitlements (`allow-jit`). |
| `scripts/ci/check-release-pipeline.sh` | Fast structural check run by the `packaging` job in `rust.yml` on every change. |

The Linux `Depends` field is not a maintained list: the script resolves the
release binary's `ldd` output and maps each library to its owning package with
`dpkg -S`, with `libgtk-3-0` and `libwebkit2gtk-4.1-0` added explicitly for
the libraries the WebView stack can load at runtime. A dependency that starts
being linked shows up in the next package without a script edit.

## Continuous validation

The packagers are not only exercised at release time. The `packaging` job in
`rust.yml` builds real `.deb` and `.rpm` packages with `dpkg-deb` and
`rpmbuild` from a stand-in binary, `packaging-macos` builds and verifies a
dmg with `hdiutil`, and the Windows job builds an MSI with WiX. The release
jobs then swap in the built `bsh`. That keeps the control files, RPM spec,
WiX source and PowerShell packager honest between releases; it deliberately
does not replace launching the app.

## Platform support and known gaps

- **macOS 11.0+**, both architectures. The deployment target is pinned by
  `MACOSX_DEPLOYMENT_TARGET=11.0` in the workflow and mirrored in
  `LSMinimumSystemVersion`. Raising one without the other is a bug.
- **Linux x86_64** is built on Ubuntu 22.04 (glibc 2.35 floor: Ubuntu 22.04+,
  Debian 12+) and ships a `.deb`, an `.rpm` and a portable tarball; it needs a
  Wayland or X11 session plus WebKitGTK 4.1. **Linux arm64** is built on Ubuntu
  24.04 (glibc 2.39 floor) but is best-effort: the free arm64 runner is
  periodically preempted, so the release may omit it. The RPM's `Requires`
  come from rpmbuild's soname generator and the package is unsigned. No
  AppImage is produced: it would have to bundle WebKitGTK and GTK, which is a
  different project from packaging this binary.
- **Windows x64**. The **terminal pane is unavailable**: the vendored Ghostty
  binding implements macOS and Wayland only and returns "libghostty native
  surfaces require macOS or Wayland" on Windows. The browser pane needs the
  Edge WebView2 runtime (present on Windows 10/11), and the MSVC build needs
  the Microsoft Visual C++ Redistributable if `vcruntime140.dll` is not already
  on the machine. `bsh.exe` is a console-subsystem binary, but when it is
  launched without a terminal (the Start Menu shortcut) it hides the console
  Windows created for it, so only a brief flash remains. Started from a shell
  the console stays, which is what `bsh ext` needs for its output; a
  GUI-subsystem build would remove the flash entirely. The executable embeds
  the same `.ico` plus version metadata, so Explorer, the taskbar and
  Add/Remove Programs show BSH instead of a generic icon.
- The release jobs **build and smoke-test each binary** (`bsh --help`, which
  does not create a window) and the `verify` job runs the workspace test suite
  under Xvfb, but no job launches the GUI. Driving a GPUI window on a shared CI
  runner is flaky and proves little about the package; GUI behaviour is covered
  by the suite and the development loop.

## Troubleshooting

- **`release version X does not match workspace version Y`** — bump
  `[workspace.package] version` in the same commit the tag points at.
- **Ghostty/Zig build error** — the vendored Ghostty pins Zig 0.16. The workflow
  installs 0.16.0 explicitly; a self-hosted runner with a newer `zig` first on
  `PATH` fails with "Your Zig version ... does not meet the required build
  version".
- **`libwebkit2gtk-4.1` not found** — the build needs the `-dev` package; the
  `ubuntu-22.04` image has it in `universe`.
- **Disk exhaustion on macOS** — a release build (Ghostty plus an LTO link) is
  large. The workflow keeps the seven-day artifact retention and relies on the
  runner's ~14 GB free; a self-hosted runner needs more.
- **`candle.exe not found`** — the Windows image normally carries WiX 3.14;
  the workflow installs it with Chocolatey if it is missing.
- **`rpmbuild` not found** — install the `rpm` package on the builder; the
  script skips the RPM with a warning when it is absent, and the release job
  installs it on both Linux images.
- **The arm64 runner is preempted** — `linux-arm64` uses `ubuntu-24.04-arm`;
  GitHub's free arm64 runners periodically shut down mid-build. The job is
  `continue-on-error` and its artifacts are optional, so the release proceeds
  without them. `macos-x86_64` uses `macos-15-intel`; on a plan without that
  label, drop that matrix entry rather than cross-compiling native deps.
- **Release job says an artifact is missing** — the build job's packaging step
  failed or the file name changed. Update the expected list in
  `release.yml` together with the script; that check is what keeps a
  half-published release from going out.
- **A dmg fails to create on a self-hosted Mac** — `hdiutil` reports
  "Resource busy" when a stale image is mounted; detach it first.

## Rolling back

Tags are immutable points of history; do not move one. To withdraw a bad build,
delete the GitHub release (the tag can stay), or publish a fixed patch version.
Re-running the workflow for the same tag rebuilds and replaces the release
assets, which is safe only when the tag still points at the same commit.
