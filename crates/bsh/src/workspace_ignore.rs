//! The workspace's own ignore file, .bsh/.ignore, and the paths the file
//! tools always exclude.
//!
//! The file is written in .gitignore syntax and read by every file tool -
//! list_dir, read_file, read_files, write_file, edit_file, apply_patch, glob
//! and grep - so a workspace can hide the trees its reader does not want the
//! model to see or touch. It is not a security boundary: the tool policy still
//! decides every path, and this file only narrows what the file tools offer.
//! .bsh and .bos are excluded by convention on top of it, because they carry
//! the harness's own state.
//!
//! The file is created on the first file-tool call, with every common tree
//! COMMENTED OUT: a fresh workspace changes nothing until its reader uncomments
//! a line. An existing file is never overwritten, so edits survive every call.

use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

/// The directory BSH and BOS use for their own workspace state.
pub(crate) const BSH_DIR: &str = ".bsh";

/// The ignore file's name inside BSH_DIR.
pub(crate) const IGNORE_NAME: &str = ".ignore";

/// The directory BOS uses for its own state.
pub(crate) const BOS_DIR: &str = ".bos";

/// The ceiling on the ignore file the tools parse.
///
/// A pattern list is a few dozen lines; a file past this is either a mistake or
/// a paste of unrelated data, and refusing to parse it keeps one workspace from
/// slowing every file tool.
const MAX_IGNORE_BYTES: u64 = 256 * 1024;

/// The file written into a workspace that has none.
///
/// Every pattern is commented out on purpose: creating the file must not change
/// what the tools do. The list is the usual dependency, version-control,
/// editor, build and cache trees, with node_modules, __pycache__ and .venv
/// named first. .bsh and .bos are noted but need no rule - they are excluded
/// unconditionally in code.
pub(crate) const DEFAULT_IGNORE: &str = r#"# =============================================================================
# BSH file-tool ignore rules
# =============================================================================
#
# The workspace file tools (list_dir, read_file, read_files, write_file,
# edit_file, apply_patch, glob, grep) skip every path that matches a pattern
# here. The format is .gitignore's: one pattern per line, # starts a comment,
# a trailing / matches directories only, a leading / anchors a pattern to the
# workspace root, * and ** glob, and a leading ! re-includes a path.
#
# Patterns are relative to the workspace root (the folder that holds .bsh).
# The common trees below are COMMENTED OUT on purpose: a fresh workspace
# filters nothing. Delete the leading # from a line to start ignoring it.
#
# .bsh and .bos are always excluded from the file tools and need no rule here.
# This file is BSH's own: the model cannot read or edit .bsh, so change it here.
# =============================================================================

# Dependencies, virtualenvs and Python caches
# node_modules
# __pycache__
# .venv
# venv
# vendor
# .pytest_cache
# .mypy_cache
# .tox

# Version control
# .git
# .jj
# .svn
# .hg

# Editors and OS metadata
# .vscode
# .idea
# .DS_Store

# Build output and caches
# target
# dist
# build
# out
# coverage
# .next
# .nuxt
# .svelte-kit
# .turbo
# .vite
# .cache
# .gradle

# Local logs and temporary files
# *.log
# *.tmp
# *.swp
"#;

/// The parsed rules for one workspace, or an empty set when there is no file.
///
/// load is called once per tool call rather than once per launch on purpose:
/// the file is a small text file, and reading it at the call is what makes an
/// edit take effect on the next tool call instead of after a restart.
#[derive(Clone, Debug)]
pub(crate) struct Rules {
    root: PathBuf,
    matcher: Option<Gitignore>,
}

impl Rules {
    /// Read the workspace's ignore file, creating it when it is missing.
    ///
    /// A workspace root that is not a directory, a .bsh that is not a real
    /// directory (a symlink out of the workspace, for example), and an
    /// unreadable file all degrade to "no rules": the file is a convenience,
    /// and a tool call must not fail because it cannot be read.
    pub(crate) fn load(root: &Path) -> Self {
        let mut rules = Self {
            root: root.to_path_buf(),
            matcher: None,
        };
        let Some(file) = ensure(root) else {
            return rules;
        };
        let Ok(meta) = fs::metadata(&file) else {
            return rules;
        };
        if meta.len() > MAX_IGNORE_BYTES {
            log::warn!(
                "workspace ignore file {} is {} bytes, past the {MAX_IGNORE_BYTES} byte ceiling; ignoring it",
                file.display(),
                meta.len()
            );
            return rules;
        }
        let mut builder = GitignoreBuilder::new(root);
        if let Some(error) = builder.add(&file) {
            log::warn!(
                "workspace ignore file {} has a bad rule: {error}",
                file.display()
            );
        }
        match builder.build() {
            Ok(matcher) => rules.matcher = Some(matcher),
            Err(error) => log::warn!(
                "workspace ignore file {} could not be compiled: {error}",
                file.display()
            ),
        }
        rules
    }

    /// The absolute path of the file these rules came from.
    pub(crate) fn path(&self) -> PathBuf {
        self.root.join(BSH_DIR).join(IGNORE_NAME)
    }

    /// Whether path is inside .bsh or .bos under the workspace root.
    ///
    /// Checked before the pattern set: these two directories hold the harness's
    /// own state, so no ignore file can make them visible and no pattern is
    /// needed to hide them.
    pub(crate) fn is_reserved(&self, path: &Path) -> bool {
        let Ok(relative) = path.strip_prefix(&self.root) else {
            return false;
        };
        relative.components().any(|component| {
            matches!(
                component,
                Component::Normal(name) if name == BSH_DIR || name == BOS_DIR
            )
        })
    }

    /// Whether path itself, or one of its parents under the workspace root, is
    /// excluded by the ignore file.
    ///
    /// A path outside the workspace is never excluded here; the tool policy
    /// answers that question.
    pub(crate) fn is_ignored(&self, path: &Path, is_dir: bool) -> bool {
        if self.is_reserved(path) {
            return true;
        }
        let Some(matcher) = &self.matcher else {
            return false;
        };
        if path == self.root || !path.starts_with(&self.root) {
            return false;
        }
        matcher
            .matched_path_or_any_parents(path, is_dir)
            .is_ignore()
    }

    /// Whether a direct child of a directory already accepted is excluded.
    ///
    /// The caller has established that the parent is not ignored, so only the
    /// child's own name and the patterns anchored at it need checking. This is
    /// the per-row cost of a listing, without walking every ancestor again.
    pub(crate) fn is_ignored_child(&self, path: &Path, is_dir: bool) -> bool {
        if self.is_reserved(path) {
            return true;
        }
        let Some(matcher) = &self.matcher else {
            return false;
        };
        if !path.starts_with(&self.root) {
            return false;
        }
        matcher.matched(path, is_dir).is_ignore()
    }

    /// The sentence a tool returns for an excluded path.
    ///
    /// Two causes get two sentences: the reader's own rule file, and the two
    /// directories the tools always exclude. Both say what the model can do,
    /// because the file itself is inside .bsh and the model cannot edit it.
    pub(crate) fn refusal(&self, path: &Path) -> String {
        if self.is_reserved(path) {
            return format!(
                "{} is inside {BSH_DIR} or {BOS_DIR}, which the file tools always exclude; use the app's own panes for that state",
                path.display()
            );
        }
        format!(
            "{} is excluded by the workspace ignore file {}; choose another path or ask the reader to uncomment its pattern",
            path.display(),
            self.path().display()
        )
    }
}

/// Create <root>/.bsh/.ignore with DEFAULT_IGNORE when it is missing.
///
/// Returns the file path when one exists afterwards. Every failure is a None
/// and not an error: the rules are an aid, and a read-only workspace must
/// still support every file tool.
fn ensure(root: &Path) -> Option<PathBuf> {
    if !root.is_dir() {
        return None;
    }
    let dir = root.join(BSH_DIR);
    match fs::symlink_metadata(&dir) {
        Ok(meta) => {
            // Do not write through a link or through a file named .bsh: a tool
            // call must not create state outside the workspace.
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return None;
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(&dir).ok()?;
        }
        Err(_) => return None,
    }
    let file = dir.join(IGNORE_NAME);
    match fs::symlink_metadata(&file) {
        Ok(meta) => {
            return meta.is_file().then_some(file);
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => return None,
    }
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&file)
    {
        Ok(mut handle) => {
            if handle.write_all(DEFAULT_IGNORE.as_bytes()).is_err() {
                // A partial file would parse as a half-written rule list, so
                // remove it and let the next call try again.
                drop(handle);
                let _ = fs::remove_file(&file);
                return None;
            }
            Some(file)
        }
        // Another tool call won the race; use the file it wrote.
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Some(file),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch workspace under target/, removed on the way in.
    fn scratch(label: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/bsh-workspace-ignore")
            .join(format!("{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch workspace");
        dir
    }

    /// The file is created with every common tree commented out, so a fresh
    /// workspace filters nothing.
    #[test]
    fn the_default_file_is_created_with_commented_patterns() {
        let root = scratch("default");
        let rules = Rules::load(&root);
        let file = root.join(".bsh").join(".ignore");
        assert_eq!(rules.path(), file);
        let text = fs::read_to_string(&file).expect("the ignore file");
        for needle in [
            "# node_modules",
            "# __pycache__",
            "# .venv",
            "# .git",
            "# .jj",
            "# .vscode",
        ] {
            assert!(
                text.contains(needle),
                "{needle} is missing from the template"
            );
        }
        // Commented out: nothing is excluded until the reader edits the file.
        assert!(!rules.is_ignored(&root.join("node_modules"), true));
        assert!(!rules.is_ignored(&root.join("src").join("main.rs"), false));
        let _ = fs::remove_dir_all(&root);
    }

    /// An existing file is used as written and never overwritten.
    #[test]
    fn a_reader_edit_survives_and_takes_effect() {
        let root = scratch("edit");
        fs::create_dir_all(root.join(".bsh")).expect(".bsh");
        let rules_text = "secret/
*.log
!keep.log
/anchored
build/
";
        fs::write(root.join(".bsh").join(".ignore"), rules_text).expect("rules");
        let rules = Rules::load(&root);
        assert_eq!(
            fs::read_to_string(rules.path()).expect("the file"),
            rules_text
        );
        assert!(rules.is_ignored(&root.join("secret"), true));
        assert!(rules.is_ignored(&root.join("secret").join("key.txt"), false));
        assert!(rules.is_ignored(&root.join("debug.log"), false));
        assert!(!rules.is_ignored(&root.join("keep.log"), false));
        assert!(!rules.is_ignored_child(&root.join("keep.log"), false));
        assert!(!rules.is_ignored(&root.join("src").join("main.rs"), false));
        // A leading slash anchors to the workspace root, and a trailing slash
        // only matches a directory.
        assert!(rules.is_ignored(&root.join("anchored"), false));
        assert!(!rules.is_ignored(&root.join("sub").join("anchored"), false));
        assert!(rules.is_ignored(&root.join("build"), true));
        assert!(!rules.is_ignored(&root.join("build"), false));
        assert!(rules.is_ignored(&root.join("build").join("out.o"), false));
        let _ = fs::remove_dir_all(&root);
    }

    /// .bsh and .bos are excluded even when the ignore file is empty, and a
    /// path outside the workspace is not this file's decision.
    #[test]
    fn reserved_directories_are_always_excluded() {
        let root = scratch("reserved");
        fs::create_dir_all(root.join(".bsh")).expect(".bsh");
        fs::write(root.join(".bsh").join(".ignore"), "").expect("empty rules");
        let rules = Rules::load(&root);
        assert!(rules.is_ignored(&root.join(".bsh").join("settings.toml"), false));
        assert!(rules.is_ignored(&root.join(".bos").join("conf"), true));
        assert!(rules.is_ignored(&root.join("sub").join(".bsh").join("x"), false));
        assert!(!rules.is_ignored(&root.join("src"), true));
        assert!(!rules.is_ignored(&root, true));
        let outside = scratch("reserved-outside");
        assert!(!rules.is_ignored(&outside.join("plain.txt"), false));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    /// A .bsh that is a symlink is not followed for the CREATE path, so a
    /// workspace cannot make a read-only tool write outside itself.
    #[test]
    #[cfg(unix)]
    fn a_symlinked_bsh_directory_is_not_written_through() {
        let root = scratch("symlink");
        let outside = scratch("symlink-outside");
        std::os::unix::fs::symlink(&outside, root.join(".bsh")).expect("link");
        let rules = Rules::load(&root);
        assert!(
            !outside.join(".ignore").exists(),
            "nothing was created outside"
        );
        assert!(!rules.is_ignored(&root.join("plain.txt"), false));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }
}
