# BSH platform support matrix

Every tool and runtime that touches the operating system goes through the
shared **hostkit** crate (`crates/hostkit`) for process-tree ownership, path
safety, environment baselines and private files. Platform support is a property
of that runtime, not of whether a crate compiles. This matrix is the contract
(issue #313); the conformance tests in `crates/hostkit/src` and the runtime
test modules enforce it on each host.

## Shared OS services (hostkit)

| Service | macOS | Linux | Windows | Mechanism |
|---|---|---|---|---|
| Process-tree ownership (`proc`) | yes | yes | yes | Unix: fresh process group; Windows: kill-on-close Job Object, created suspended/assigned/resumed |
| Termination confirmation | yes | yes | yes | group/job emptiness confirmed before "killed" is published |
| Process inspection (`proc::process_info`) | yes (ps) | yes (/proc) | yes (toolhelp + PEB) | parent pid + command line; unreadable = None, never a guess |
| Path safety (`paths`) | yes | yes | yes | one validator rejects absolute/drive/UNC/device/ADS/traversal on every host |
| Environment baseline (`env`) | yes | yes | yes | Unix allowlist; Windows native vars (SystemRoot, profile, temp) case-insensitive |
| User environment (`env::user_environment`) | process env | process env | process env + HKCU\Environment | native registry read, bounded, falls back to process env |
| Private files (`fspriv`) | 0600 | 0600 | protected owner-only DACL | fail-closed; applied at creation and on replacement |
| Home/root resolution (`home`) | yes | yes | yes | OS-native home; absolute override wins; relative anchored to home, never CWD |

## Agent tools

| Tool | macOS | Linux | Windows | Notes |
|---|---|---|---|---|
| BashTool (`bash`) | yes | yes | yes (Git Bash / WSL) | POSIX dialect; Windows resolves an installed Bash runtime or refuses with an actionable error; never reinterpreted as CMD/PowerShell. WSL is refused under confinement (no AppContainer on the Linux side). |
| File tools (read/write/edit/glob/grep) | yes | yes | yes | workspace validator hardened for Windows path forms (#308) |
| Background jobs (`job_*`) | yes | yes | yes | tree termination confirmed before "killed" (#312) |
| External hooks | yes | yes | yes | run through the process runtime with a bounded timeout and tree kill (#319) |
| MCP tools | yes | yes | yes | per-server/per-tool governance before invocation (#330); bounded call deadline + retirement (#331); tree cleanup (#315); native env (#318) |
| Sandbox (heel) | yes (SBPL) | yes (Landlock) | yes (AppContainer) | one implementation; a backend that cannot confine refuses rather than running unconfined |

## BSH surfaces

| Surface | macOS | Linux | Windows | Notes |
|---|---|---|---|---|
| Browser engine | yes | yes | yes | Windows Chrome/Chromium discovery (#326); fail-closed sandbox, no automatic --no-sandbox (#325); graceful tree shutdown (#327); native orphan reaper (#314); turn egress policy on navigation (#329) |
| Terminal pane | yes | Wayland only | not yet | libghostty native surface is macOS/Wayland; X11 and Windows show an actionable platform message before surface creation (#296, #297). A confined turn requires an explicit release before an unconfined shell starts (#323). |
| Host terminal service (extensions) | sh | sh | PowerShell / cmd | native shell, dialect documented (#298) |
| Settings / vault / secrets | yes | yes | yes | atomic multi-document commit (#310); owner-only secret files (#299) |
| Extension market / installer | yes | yes | yes | cache identity from validated repository identity (#305); git subpath confinement (#322); MCP config confinement (#321) |

## Ferris runtimes

| Runtime | macOS | Linux | Windows | Notes |
|---|---|---|---|---|
| Process runtime | yes | yes | yes | Job Object integrated into ProcessSession (#316) |
| Cordis bridge | yes | yes | yes | Node process-tree ownership (#320); drive-letter entry path fixed (#304); hook dispatch (#300) |
| MCP runtime | yes | yes | yes | tree cleanup + native env (#315, #318) |
| Services | yes | yes | yes | shared-scope publication with per-provider revocation (#302); realm-aware contract acquisition (#301) |

## Conformance and CI

- `cargo test -p hostkit` runs the shared OS-service conformance suite on every
  host CI runs on (process ownership/termination, path safety, environment,
  private files, root resolution).
- The Windows code paths are compile-verified in CI via
  `cargo check -p hostkit --target x86_64-pc-windows-msvc` (no link needed).
- Runtime lifecycle tests (jobs, hooks, MCP, process runtime, bridge, browser
  reaper) run on the host CI lane; the process-lifecycle contract is the same
  on all three platforms because every runtime spawns through hostkit.

## Honest limits

- The Windows terminal pane has no native backend yet; it is explicitly gated,
  not silently broken (#296).
- Runtime behavior on Windows is compile-verified and unit-tested where the
  host allows; full Windows runtime integration runs on the Windows CI lane.
- WSL Bash cannot be confined by the Windows AppContainer backend; a confined
  turn refuses it rather than claiming an isolation it does not have.
