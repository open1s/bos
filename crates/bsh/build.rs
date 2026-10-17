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
}
