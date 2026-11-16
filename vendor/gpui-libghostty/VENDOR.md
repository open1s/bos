# Vendored gpui-libghostty

- Upstream: https://github.com/behzade/gpui-libghostty
- Version: 0.3.1 (crates.io package `gpui-libghostty-0.3.1`)
- License: MIT. The bundled Ghostty source is MIT; see
  `vendor/ghostty/LICENSE` and `vendor/ghostty/VENDOR.md`.

## Why a copy lives here

The crate's `build.rs` compiles its Objective-C shim with the clang inside
`<xcode-select -p>/Toolchains/XcodeDefault.xctoolchain/usr/bin`. A machine with
only the **Xcode Command Line Tools** (no full Xcode) has no such directory:
its clang and ar live under `<xcode-select -p>/usr/bin`, so the build stops with
`No such file or directory` before it ever links. bsh's authoring machine is
CLT-only.

## Local patches

1. `build.rs`, `NativeTools::toolchain`: keep the Xcode path first, then fall
   back to `<developer_dir>/usr/bin`, then `/usr/bin`. A Command-Line-Tools-only
   machine has no `XcodeDefault.xctoolchain` at all.
2. `build.rs`, `resolve_zig`: the vendored Ghostty pins Zig 0.16 exactly, and a
   shell whose PATH starts with a newer Homebrew zig fails the build. ZIG still
   wins when set; otherwise a 0.16 toolchain (from PATH or a mise install) is
   preferred over whatever `zig` resolves to.

Nothing else differs from the published 0.3.1 package.

## Repository note

Two vendored files exceed jj's default 1 MiB snapshot limit
(`vendor/ghostty/pkg/simdutf/vendor/simdutf.cpp`, 1.7 MiB, and
`vendor/ghostty/pkg/wuffs/src/too_big.png`, 1.0 MiB). Committing this tree with
jj needs the limit raised once per clone:

    jj config set --repo snapshot.max-new-file-size 2000000

Drop the vendored copy and go back to the registry dependency (the comment in
`crates/bsh/Cargo.toml` says how) once upstream accepts the same fallback.
