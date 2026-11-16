//! Rebuild when a locale file changes.
//!
//! `rust_i18n::i18n!` embeds `locales/*.yml` at compile time. Nothing else in
//! the crate names those files, so without this script cargo sees no reason to
//! rebuild when one is edited and the running binary keeps the strings it was
//! compiled with. That turned a translated label into a raw key in the window
//! while both the file and the call site read correct, which is a silent
//! failure: the edit looks applied and is not.
//!
//! A build script is the only seam that can say "this crate depends on that
//! data file", so the dependency is declared here rather than hoped for.

fn main() {
    println!("cargo:rerun-if-changed=locales/en.yml");
    println!("cargo:rerun-if-changed=locales/zh.yml");
    println!("cargo:rerun-if-changed=locales");
    println!("cargo:rerun-if-changed=src/assets/bos-harness-icon.ico");

    #[cfg(windows)]
    embed_windows_resources();
}

/// Compile the icon and version metadata into the Windows executable.
///
/// A Windows binary with no resource section gets the generic blank icon in
/// Explorer, the taskbar and the Start Menu, regardless of what the installer
/// or the .ico file says. The version fields also give Add/Remove Programs
/// and the file's Properties page real values instead of zeros.
///
/// This is host-gated: the Windows build is native on the release runner, so a
/// build script running on Windows is building for Windows. Cross-compiling
/// from another host would need the target check instead, and is not how the
/// release is produced.
#[cfg(windows)]
fn embed_windows_resources() {
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("src/assets/bos-harness-icon.ico");
    resource.set("ProductName", "BSH");
    resource.set("FileDescription", "BOS Harness");
    resource.set("CompanyName", "BrickOS Team");
    resource.set("LegalCopyright", "Copyright (c) 2026 open1s");
    resource.set("FileVersion", env!("CARGO_PKG_VERSION"));
    resource.set("ProductVersion", env!("CARGO_PKG_VERSION"));
    if let Err(error) = resource.compile() {
        // Fail loudly rather than ship an exe with a blank icon: rc.exe comes
        // with the Windows SDK that the MSVC toolchain already needs, so a
        // failure here means the build environment is incomplete.
        panic!("embed Windows resources: {error}");
    }
}
