//! Preset path scopes: validating them at load time and matching them in Rego.
//!
//! A preset declares `read`, `write` and `deny` lists. Each entry is either a
//! workspace-relative path/glob or an absolute one:
//!
//! - a LITERAL entry (`docs`, `README.md`, `/abs/logs`) names that path and
//!   everything below it, which is the prefix behaviour every shipped preset
//!   already relies on;
//! - an entry containing a glob metacharacter (`*`, `?`, `[`, `{`) is a glob:
//!   `src/**/*.rs` matches the Rust sources under `src`, and `*` never crosses
//!   `/` because the matcher is compiled with a literal separator.
//!
//! `deny` is decided FIRST, so it wins over `read` and `write` even where the
//! tier and the workspace root would allow the path. `read`/`write` are a
//! whitelist: a declared scope is an intersection with the tier, never a union,
//! and an absent one inherits the workspace root.
//!
//! Validation is a LOAD decision, not a matching decision: an empty entry, a
//! `~` path, a `..` component, or an unparsable glob is refused by name (the
//! preset id and the field), so a typo can never silently fall back to "allow
//! everything" or be dropped from the deny list.

use super::set::PolicySource;

/// The largest number of brace/bracket nesting levels a scope glob may use.
///
/// A deeper pattern is not useful for a file path and is a cheap way to make a
/// matcher do exponential work, so it is refused at load instead.
const MAX_GLOB_NESTING: usize = 8;

/// Validate one preset path-scope entry.
///
/// The message is the reason the reader sees beside the field name; it never
/// suggests a silent fallback.
pub fn validate_scope_entry(entry: &str) -> Result<(), String> {
    let value = entry.trim();
    if value.is_empty() {
        return Err("a path scope entry may not be empty".to_string());
    }
    if value.contains('\0') {
        return Err("a path scope entry may not contain a NUL byte".to_string());
    }
    if value.starts_with('~') {
        return Err(
            "a '~' path is not expanded in a preset scope; write it relative to the workspace root or absolute"
                .to_string(),
        );
    }
    if value.split('/').any(|component| component == "..") {
        return Err("a path scope entry may not contain a '..' component".to_string());
    }
    validate_glob(value)?;
    Ok(())
}

/// Validate the glob syntax of one scope entry.
///
/// This is the same surface the matcher (`glob.match`) accepts: balanced `[`
/// classes and `{` alternations, and `**` only as a whole path component.
fn validate_glob(pattern: &str) -> Result<(), String> {
    let mut brackets: Vec<usize> = Vec::new();
    let mut braces: Vec<usize> = Vec::new();
    for (index, ch) in pattern.char_indices() {
        match ch {
            '[' => {
                if brackets.len() >= MAX_GLOB_NESTING {
                    return Err(format!(
                        "the glob nests character classes more than {MAX_GLOB_NESTING} deep"
                    ));
                }
                brackets.push(index);
            }
            ']' => {
                if brackets.pop().is_none() {
                    return Err(
                        "the glob closes a character class that was never opened".to_string()
                    );
                }
                if let Some(open) = brackets.last() {
                    // A class must not be empty: `[]` is refused by the matcher.
                    if *open + 1 == index {
                        return Err("the glob carries an empty character class".to_string());
                    }
                }
            }
            '{' => {
                if braces.len() >= MAX_GLOB_NESTING {
                    return Err(format!(
                        "the glob nests alternations more than {MAX_GLOB_NESTING} deep"
                    ));
                }
                braces.push(index);
            }
            '}' => {
                if braces.pop().is_none() {
                    return Err("the glob closes an alternation that was never opened".to_string());
                }
                if let Some(open) = braces.last() {
                    if *open + 1 == index {
                        return Err("the glob carries an empty alternation".to_string());
                    }
                }
            }
            _ => {}
        }
    }
    if !brackets.is_empty() {
        return Err("the glob leaves a character class unclosed".to_string());
    }
    if !braces.is_empty() {
        return Err("the glob leaves an alternation unclosed".to_string());
    }
    // globset refuses a recursive wildcard that is not a whole component:
    // `a**b` is an error, `src/**/*.rs` is not.
    for component in pattern.split('/') {
        if component.contains("**") && component != "**" {
            return Err(
                "a '**' recursive wildcard must be a whole path component (for example 'src/**/*.rs')"
                    .to_string(),
            );
        }
    }
    Ok(())
}

/// Whether one entry is a glob rather than a literal path.
pub fn is_glob(entry: &str) -> bool {
    entry.chars().any(|ch| matches!(ch, '*' | '?' | '[' | '{'))
}

/// The preset path-scope module: enforced only when the preset declares a
/// read, write or deny scope. It may only narrow the baseline.
///
/// It reads two parallel shapes per dimension: LITERAL roots (prefix matches,
/// which is what `docs` has always meant) and GLOB patterns (matched with
/// `glob.match`, where `*` does not cross a separator and `**` does).
pub const PRESET_SCOPE_REGO: &str = r#"package policy.preset_scope

# rule: preset-deny-path
verdict := {"allow": false, "rule": "preset-deny-path", "reason": "the path is on the preset's deny list"} if {
    input.target.kind == "path"
    denied_path(input.target.path)
# rule: preset-read-scope
} else := {"allow": false, "rule": "preset-read-scope", "reason": "the path is outside the preset's read scope"} if {
    input.action == "read"
    input.target.kind == "path"
    data.preset_scope.read_restricted
    not read_allowed(input.target.path)
# rule: preset-write-scope
} else := {"allow": false, "rule": "preset-write-scope", "reason": "the path is outside the preset's write scope"} if {
    input.action == "write"
    data.preset_scope.write_restricted
    not write_allowed(input.target.path)
} else := {"allow": true}

denied_path(path) if {
    some root in data.preset_scope.deny_roots
    path_under(path, root)
}

denied_path(path) if {
    some pattern in data.preset_scope.deny_globs
    glob.match(pattern, ["/"], path)
}

read_allowed(path) if {
    some root in data.preset_scope.read_roots
    path_under(path, root)
}

read_allowed(path) if {
    some pattern in data.preset_scope.read_globs
    glob.match(pattern, ["/"], path)
}

write_allowed(path) if {
    some root in data.preset_scope.write_roots
    path_under(path, root)
}

write_allowed(path) if {
    some pattern in data.preset_scope.write_globs
    glob.match(pattern, ["/"], path)
}

path_under(path, root) if {
    root == "/"
    startswith(path, "/")
}

path_under(path, root) if {
    root != "/"
    root != ""
    startswith(concat("", [path, "/"]), concat("", [root, "/"]))
}
"#;

/// The preset scope module as a source.
pub fn preset_scope_source() -> PolicySource {
    PolicySource::builtin("builtin:preset-scope", PRESET_SCOPE_REGO)
}
