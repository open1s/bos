//! `/init`: generate an `AGENTS.md` for the project (codex parity).
//!
//! The instruction loader (see [`crate::instructions`]) reads `AGENTS.md`
//! from the bash workspace and prepends it to every turn; `/init` is the
//! bootstrap that writes that file. A tool-less probe agent gets a shallow
//! directory tree of the project and drafts the document — no bash, file,
//! memory, MCP, or skill tools, so generation can never side-effect the
//! workspace beyond the single `AGENTS.md` write performed here.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::approval::ApprovalBroker;
use crate::caps;
use crate::runner::Runner;
use crate::settings::Settings;

/// Deadline for the generation turn: a full AGENTS.md is a long-form
/// answer (thousands of tokens), so this is deliberately generous — the
/// turn runs on a background thread behind the status chip either way.
const GENERATE_TIMEOUT: Duration = Duration::from_secs(600);
/// Maximum tree lines embedded in the prompt.
const MAX_TREE_LINES: usize = 400;
/// Directory depth shown in the prompt (root entries plus one level).
const MAX_TREE_DEPTH: usize = 2;
/// Directory names never listed nor recursed into.
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".jj",
    ".hg",
    "target",
    "node_modules",
    "dist",
    "build",
    "__pycache__",
    ".venv",
    "vendor",
];

/// System prompt for the generation turn (tools, skills, memory, MCP, and
/// the project's own instructions are all disabled for this probe agent).
pub(crate) const INIT_SYSTEM: &str = "\
You write AGENTS.md files: the high-signal instruction document a coding \
agent reads before touching a repository. Reply with ONLY the Markdown body \
of the file — no preamble, no code fences around the whole reply, no closing \
remarks. Cover what the project is, its layout, the real build/test/lint \
commands, conventions, and pitfalls; omit anything the given tree does not \
support instead of guessing. Be terse and factual; prefer bullets; stay \
under 200 lines.";

/// Where the generated document lands inside `root`.
pub(crate) fn agents_path(root: &Path) -> PathBuf {
    root.join("AGENTS.md")
}

/// Why `/init` should not run in `root`, or `None` when it may.
///
/// An existing file is never clobbered unless `force` is set (codex parity),
/// and the workspace must be a real directory.
pub(crate) fn refusal(root: &Path, force: bool) -> Option<String> {
    if !root.is_dir() {
        return Some(format!(
            "bash workspace is not a directory: {}",
            root.display()
        ));
    }
    if !force && agents_path(root).exists() {
        return Some(format!(
            "{} already exists — delete it (or pass force) to regenerate",
            agents_path(root).display()
        ));
    }
    None
}

/// Walk `root` for a compact two-level tree: sorted, budget-capped, and
/// without hidden or heavyweight directories (`target`, `.git`, …).
pub(crate) fn tree(root: &Path) -> String {
    let mut lines = Vec::new();
    let mut budget = MAX_TREE_LINES;
    walk(root, 0, &mut lines, &mut budget);
    if budget == 0 {
        lines.push("…".to_string());
    }
    lines.join("\n")
}

fn walk(dir: &Path, depth: usize, lines: &mut Vec<String>, budget: &mut usize) {
    // An entry emitted at walk depth `d` is tree level `d + 1`; only levels
    // 1..=MAX_TREE_DEPTH are listed, so a walk starting at MAX_TREE_DEPTH
    // has nothing left to emit (its parent already listed it).
    if *budget == 0 || depth >= MAX_TREE_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<_> = entries.flatten().collect();
    names.sort_by_key(|e| e.file_name());
    for entry in names {
        if *budget == 0 {
            break;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        let path = entry.path();
        let is_dir = path.is_dir();
        if is_dir && depth == MAX_TREE_DEPTH {
            continue; // show directory names, but do not descend further
        }
        lines.push(format!(
            "{}{}",
            "  ".repeat(depth),
            if is_dir {
                format!("{name}/")
            } else {
                name.clone()
            }
        ));
        *budget -= 1;
        if is_dir {
            walk(&path, depth + 1, lines, budget);
        }
    }
}

/// The generation user message: contract plus the rendered project tree.
pub(crate) fn prompt(root: &Path, tree: &str) -> String {
    format!(
        "Create an AGENTS.md for the project rooted at {}.\n\n\
         Output only the Markdown body of the file: no code fences wrapping \
         the reply, no preamble, no sign-off.\n\n\
         Project tree (two levels; build/test commands must be inferred from \
         these names, never invented):\n\n{}",
        root.display(),
        tree
    )
}

/// Drop an accidental all-encompassing code fence the model sometimes wraps
/// the whole document in; plain bodies pass through untouched.
pub(crate) fn strip_fences(body: &str) -> String {
    let trimmed = body.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_string();
    }
    let mut lines = trimmed.lines();
    lines.next(); // the opening fence
    let inner: Vec<&str> = lines.collect();
    if let Some(last) = inner.last() {
        if last.trim_start().starts_with("```") {
            let mut inner = inner;
            inner.pop();
            return inner.join("\n").trim().to_string();
        }
    }
    trimmed.to_string()
}

/// Run one tool-less generation turn over the project tree and return the
/// `AGENTS.md` body.
///
/// The probe agent reuses the endpoint/model from `settings` but registers
/// no bash, file, memory, MCP, or skill tools and no project instructions —
/// generation must never side-effect the workspace.
pub(crate) fn generate(settings: &Settings, root: &Path) -> Result<String, String> {
    let mut probe = settings.clone();
    probe.system_prompt = INIT_SYSTEM.to_string();
    probe.project_instructions = false;
    probe.bash_enabled = false;
    probe.file_tools_enabled = false;
    probe.memory_enabled = false;
    probe.skills_dir.clear();
    probe.mcp_servers.clear();
    probe.require_approval = false;

    let tree = tree(root);
    // A long-form turn occasionally ends with zero content when the provider
    // resets the stream mid-generation, so retry exactly that case once.
    for _attempt in 0..2u8 {
        let agent = caps::build_agent(&probe, Arc::new(ApprovalBroker::default()));
        let mut runner = Runner::new();
        let rx = runner.spawn("__init__", agent, prompt(root, &tree), Vec::new(), 0);
        let text = runner.collect(&rx, GENERATE_TIMEOUT)?;
        let body = strip_fences(&text);
        if !body.is_empty() {
            return Ok(body);
        }
    }
    Err("the model returned no text (twice)".to_string())
}

/// Write `body` to `AGENTS.md` inside `root`, returning the file path.
pub(crate) fn write(root: &Path, body: &str) -> Result<PathBuf, String> {
    let path = agents_path(root);
    std::fs::write(&path, body)
        .map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bos-gui-init-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn refusal_blocks_existing_unless_forced() {
        let root = tmp_dir("exists");
        assert!(refusal(&root, false).is_none(), "empty root proceeds");

        std::fs::write(agents_path(&root), "# docs").expect("write");
        let reason = refusal(&root, false).expect("existing file must refuse");
        assert!(reason.contains("AGENTS.md"), "{reason}");
        assert!(refusal(&root, true).is_none(), "force allows regeneration");

        let missing = root.join("nope");
        let reason = refusal(&missing, false).expect("missing dir must refuse");
        assert!(reason.contains("not a directory"), "{reason}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tree_lists_two_levels_and_skips_junk() {
        let root = tmp_dir("tree");
        std::fs::create_dir_all(root.join("src/deep")).expect("dirs");
        std::fs::create_dir_all(root.join(".git")).expect("git");
        std::fs::create_dir_all(root.join("target/debug")).expect("target");
        std::fs::write(root.join("README.md"), "hi").expect("file");
        std::fs::write(root.join("src/lib.rs"), "fn main() {}").expect("file");
        std::fs::write(root.join("src/deep/skip.rs"), "x").expect("file");

        let tree = tree(&root);
        assert!(tree.contains("README.md"), "{tree}");
        assert!(tree.contains("src/"), "{tree}");
        // Children render indented one level under their directory.
        assert!(tree.contains("\n  lib.rs"), "{tree}");
        assert!(!tree.contains(".git"), "{tree}");
        assert!(!tree.contains("target"), "{tree}");
        assert!(!tree.contains("skip.rs"), "depth-3 files stay out: {tree}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn prompt_carries_the_tree_and_the_contract() {
        let root = Path::new("/tmp/proj");
        let text = prompt(root, "src/\n  main.rs");
        assert!(text.contains("/tmp/proj"), "{text}");
        assert!(text.contains("AGENTS.md"), "{text}");
        assert!(text.contains("src/"), "{text}");
        assert!(text.contains("never invented"), "{text}");
    }

    #[test]
    fn strip_fences_unwraps_and_passes_plain_text() {
        assert_eq!(strip_fences("# Title\nbody"), "# Title\nbody");
        assert_eq!(
            strip_fences("```markdown\n# Title\nbody\n```"),
            "# Title\nbody"
        );
        // An unterminated opening fence is left as-is rather than eaten.
        assert!(strip_fences("```\nstill text").contains("still text"));
    }

    #[test]
    fn write_lands_agents_md_and_reports_the_path() {
        let root = tmp_dir("write");
        let path = write(&root, "# Guide").expect("write");
        assert_eq!(path, agents_path(&root));
        let got = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(got, "# Guide");
        let _ = std::fs::remove_dir_all(&root);
    }
}
