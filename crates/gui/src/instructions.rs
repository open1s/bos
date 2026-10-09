//! Project-instruction files (AGENTS.md-style) for the chat agent.
//!
//! Codex and DeepSeek Harness both fold repository instruction files into
//! the system prompt; this module is the GUI-side equivalent. When settings
//! enable it, every agent built for a workspace root receives the contents
//! of the well-known files under that root **and under its ancestor
//! directories** (the codex parent-walk), so machine- or user-level rules
//! reach every model turn without user re-pasting.
//!
//! Design rules:
//! - Only the fixed well-known names are read, directly in each directory —
//!   no traversal into subdirectories or siblings.
//! - The walk starts at the root and climbs at most [`MAX_ANCESTOR_LEVELS`]
//!   parents, stopping at the home directory (inclusive) when the root lives
//!   under it, so a stray `/AGENTS.md` on the way to the filesystem root is
//!   never picked up.
//! - Closest wins first: the workspace root's sections appear before any
//!   ancestor's, so project rules take precedence over generic parents.
//! - Files are read as UTF-8; unreadable or non-UTF-8 files are skipped
//!   rather than failing the agent build.
//! - One shared byte budget keeps a stray huge file from blowing up the
//!   prompt; the cut is UTF-8-safe and marked as truncated.
//! - The loader is pure filesystem input — expansion still happens at
//!   build time, so edits to `AGENTS.md` apply to the next new session.

use std::fs;
use std::path::{Path, PathBuf};

/// Instruction files read from each scanned directory, in application order.
///
/// Earlier files appear first in the composed block, so `AGENTS.md` (the
/// cross-tool standard) outranks the tool-specific variants.
pub(crate) const INSTRUCTION_FILES: [&str; 3] = ["AGENTS.md", "CLAUDE.md", ".bos/instructions.md"];

/// Shared byte budget for the whole composed instruction block.
pub(crate) const MAX_INSTRUCTION_BYTES: usize = 16 * 1024;

/// How far the ancestor walk climbs above the root (root itself excluded).
///
/// The home-directory boundary usually stops the walk earlier; the cap is
/// the backstop for roots outside `$HOME`.
pub(crate) const MAX_ANCESTOR_LEVELS: usize = 8;

/// Read the well-known instruction files under `root` **and its ancestor
/// directories** and compose them into one block, or `None` when none are
/// present (empty/missing/blank files).
///
/// The scan starts at the root (its sections keep the plain relative name)
/// and climbs toward `$HOME`, stopping there inclusively or after
/// [`MAX_ANCESTOR_LEVELS`] parents. Ancestor sections are headed with their
/// full path so two files with the same basename stay distinguishable.
/// When the shared [`MAX_INSTRUCTION_BYTES`] budget runs out mid-file, the
/// content is cut on a UTF-8 char boundary and the block ends with a
/// truncation marker (which may sit slightly past the budget).
pub(crate) fn load(root: &Path) -> Option<String> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    load_scoped(root, home.as_deref())
}

/// [`load`] with an explicit home boundary — `$HOME` in production, injected
/// in tests so the stop rule stays deterministic and env-independent.
fn load_scoped(root: &Path, home: Option<&Path>) -> Option<String> {
    let mut out = String::new();
    'dirs: for dir in instruction_dirs(root, home) {
        for name in INSTRUCTION_FILES {
            if out.len() >= MAX_INSTRUCTION_BYTES {
                break 'dirs;
            }
            // Fixed well-known names only: join cannot escape `dir`.
            let Ok(text) = fs::read_to_string(dir.join(name)) else {
                continue; // missing or non-UTF-8: skip, never fail the build
            };
            if text.trim().is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str("## Instructions from ");
            if dir.as_path() == root {
                out.push_str(name);
            } else {
                out.push_str(&dir.join(name).display().to_string());
            }
            out.push_str("\n\n");
            let remaining = MAX_INSTRUCTION_BYTES.saturating_sub(out.len());
            if text.len() > remaining {
                let mut cut = remaining;
                while cut > 0 && !text.is_char_boundary(cut) {
                    cut -= 1;
                }
                out.push_str(&text[..cut]);
                out.push_str("\n… [truncated]");
                break 'dirs;
            }
            out.push_str(&text);
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Directories [`load_scoped`] scans: the root first (closest wins), then
/// ancestors nearest-first, capped at [`MAX_ANCESTOR_LEVELS`] and stopping
/// at `home` when the root lives under it. Empty components (a relative
/// root running out of parents) end the walk.
fn instruction_dirs(root: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = vec![root.to_path_buf()];
    if home == Some(root) {
        return dirs;
    }
    let mut cur = root;
    for _ in 0..MAX_ANCESTOR_LEVELS {
        let Some(parent) = cur.parent() else { break };
        if parent.as_os_str().is_empty() {
            break;
        }
        dirs.push(parent.to_path_buf());
        if Some(parent) == home {
            break;
        }
        cur = parent;
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bos-gui-instr-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Hermetic root-only load: pretending the root *is* `$HOME` stops the
    /// ancestor walk at the root, so no machine-level file can leak into a
    /// root-only assertion.
    fn load_root(root: &Path) -> Option<String> {
        load_scoped(root, Some(root))
    }

    #[test]
    fn load_returns_none_without_instruction_files() {
        let root = temp_root("none");
        assert_eq!(load_root(&root), None);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn load_orders_agents_first_and_sections_each_file() {
        let root = temp_root("order");
        fs::write(root.join("CLAUDE.md"), "claude rules").unwrap();
        fs::write(root.join("AGENTS.md"), "agents rules").unwrap();
        let block = load_root(&root).unwrap();
        let agents = block.find("## Instructions from AGENTS.md").unwrap();
        let claude = block.find("## Instructions from CLAUDE.md").unwrap();
        assert!(agents < claude, "AGENTS.md must be applied first: {block}");
        assert!(block.contains("agents rules"));
        assert!(block.contains("claude rules"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn load_skips_missing_and_blank_files() {
        let root = temp_root("blank");
        fs::write(root.join("AGENTS.md"), "   \n\t  ").unwrap();
        fs::create_dir_all(root.join(".bos")).unwrap();
        fs::write(root.join(".bos/instructions.md"), "real rules").unwrap();
        let block = load_root(&root).unwrap();
        assert!(!block.contains("AGENTS.md"), "blank file yields no section");
        assert!(block.contains("## Instructions from .bos/instructions.md"));
        assert!(block.contains("real rules"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn load_caps_total_bytes_on_a_char_boundary() {
        let root = temp_root("cap");
        // Multibyte payload: a byte-wise cut would produce invalid UTF-8.
        let big = "世界".repeat(MAX_INSTRUCTION_BYTES);
        fs::write(root.join("AGENTS.md"), big).unwrap();
        let block = load_root(&root).unwrap();
        assert!(block.contains("[truncated]"));
        assert!(block.len() < MAX_INSTRUCTION_BYTES + 64, "budget respected");
        let _ = fs::remove_dir_all(&root);
    }

    /// Parent-directory files now contribute (codex-style walk), with the
    /// workspace root's sections first: closest rules win attention.
    #[test]
    fn load_includes_parent_instructions_closest_first() {
        let container = temp_root("parent");
        let root = container.join("ws");
        fs::create_dir_all(&root).unwrap();
        fs::write(container.join("AGENTS.md"), "parent rules").unwrap();
        fs::write(root.join("CLAUDE.md"), "child rules").unwrap();
        let block = load(&root).unwrap();
        let child = block.find("child rules").unwrap();
        let parent = block.find("parent rules").unwrap();
        assert!(
            child < parent,
            "root sections come before ancestors: {block}"
        );
        let header = format!(
            "## Instructions from {}",
            container.join("AGENTS.md").display()
        );
        assert!(
            block.contains(&header),
            "ancestor header shows its path: {block}"
        );
        let _ = fs::remove_dir_all(&container);
    }

    /// The home directory is included when reached; anything above it is not.
    #[test]
    fn load_stops_above_home_boundary() {
        let base = temp_root("homebound");
        let home = base.join("home");
        let root = home.join("proj");
        fs::create_dir_all(&root).unwrap();
        fs::write(base.join("AGENTS.md"), "above home rules").unwrap();
        fs::write(home.join("AGENTS.md"), "home rules").unwrap();
        fs::write(root.join("AGENTS.md"), "project rules").unwrap();
        let block = load_scoped(&root, Some(&home)).unwrap();
        assert!(block.contains("project rules"), "{block}");
        assert!(
            block.contains("home rules"),
            "home itself is scanned: {block}"
        );
        assert!(!block.contains("above home rules"), "stop at home: {block}");
        let _ = fs::remove_dir_all(&base);
    }

    /// Without a home match the walk still stops at the level cap.
    #[test]
    fn load_caps_ancestor_walk_at_max_levels() {
        let base = temp_root("capwalk");
        let mut dir = base.clone();
        for i in 0..=MAX_ANCESTOR_LEVELS {
            dir = dir.join(format!("d{i}"));
        }
        fs::create_dir_all(&dir).unwrap(); // root at depth MAX_ANCESTOR_LEVELS + 1
        fs::write(dir.parent().unwrap().join("AGENTS.md"), "near rules").unwrap();
        // One level beyond the cap: never scanned.
        fs::write(base.join("CLAUDE.md"), "far rules").unwrap();
        let block = load_scoped(&dir, None).unwrap();
        assert!(
            block.contains("near rules"),
            "first ancestor is in: {block}"
        );
        assert!(!block.contains("far rules"), "cap respected: {block}");
        let _ = fs::remove_dir_all(&base);
    }
}
