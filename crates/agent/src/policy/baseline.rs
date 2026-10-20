//! The baseline policy shipped in-tree.
//!
//! This module is the reference example a user copies: small, readable, and
//! written against the same [`PolicyInput`](crate::policy::PolicyInput)
//! document any other source sees. It answers every action, so a workspace
//! policy can layer over it without having to restate a floor.
//!
//! In plain English:
//!
//! - **Reads** are allowed inside the workspace root and refused outside it.
//! - **Writes** are allowed under the workspace root, except anywhere below a
//!   `.bsh` or `.git` directory. A write outside the root is refused.
//! - **Egress** is allowed only when the resolved address is on
//!   `data.egress_allowlist` (an exact address, or a CIDR range). The list is
//!   empty by default, so egress is denied by default.
//! - **Spawn** is allowed only for the `bash` tool when its working directory is
//!   the workspace root. `job_kill` (stopping a job this session owns) and
//!   `subagent` (delegating to a child agent) are also spawns the baseline
//!   permits; the workspace policy narrows them. Any other process start is
//!   refused.
//! - **Capability** is the registration-time question. The four gated tools -
//!   `run_in_background`, `job_output`, `job_kill` and `subagent` - are
//!   offered only while `data.permission` grants side effects; a read-only
//!   turn refuses them in Rego as well as in the Rust matrix.
//! - Anything else falls to a default refusal.
//!
//! Containment is one `startswith` against `data.workspace_prefix` (the root
//! plus `/`, or `/` itself for a root of `/`), so the root and every
//! descendant are covered by one comparison instead of a per-component scan.
//!
//! Every denying rule carries a `# rule: <id>` comment, and the verdict repeats
//! the id in its `rule` field. The loader extracts the comments into
//! [`PolicySet::rule_ids`](crate::policy::PolicySet::rule_ids); the verdict's
//! field is what a denial names.

use super::set::PolicySource;

/// The shipped baseline Rego module.
pub const BASELINE_POLICY_REGO: &str = r#"package policy.baseline

# The baseline answers every action. A workspace policy may narrow these
# answers; it can never widen one, because this module is always evaluated too.
# Each verdict is an else chain, so the first matching branch decides and the
# engine does not evaluate the rest.

# rule: capability-granted
verdict := {"allow": true, "rule": "capability-granted"} if {
    input.action == "capability"
    gated_capability(input.tool)
    data.permission != "read-only"
# rule: capability-read-only
} else := {"allow": false, "rule": "capability-read-only", "reason": "the effective permission does not grant this tool"} if {
    input.action == "capability"
    gated_capability(input.tool)
# rule: capability-unknown
} else := {"allow": false, "rule": "capability-unknown", "reason": "no capability rule grants this tool"} if {
    input.action == "capability"
# rule: read-inside-workspace
} else := {"allow": true, "rule": "read-inside-workspace"} if {
    input.action == "read"
    inside_path(input.target.path)
# rule: read-outside-workspace
} else := {"allow": false, "rule": "read-outside-workspace", "reason": "reads are confined to the workspace"} if {
    input.action == "read"
# rule: write-forbidden-directory
} else := {"allow": false, "rule": "write-forbidden-directory", "reason": "writes into .bsh or .git internals are forbidden"} if {
    input.action == "write"
    inside_path(input.target.path)
    forbidden_path(input.target.path)
# rule: write-inside-workspace
} else := {"allow": true, "rule": "write-inside-workspace"} if {
    input.action == "write"
    inside_path(input.target.path)
# rule: write-outside-workspace
} else := {"allow": false, "rule": "write-outside-workspace", "reason": "writes are confined to the workspace"} if {
    input.action == "write"
# rule: egress-allowlisted
} else := {"allow": true, "rule": "egress-allowlisted"} if {
    input.action == "egress"
    egress_allowed(input.target.ip)
# rule: egress-not-allowlisted
} else := {"allow": false, "rule": "egress-not-allowlisted", "reason": "the resolved address is not on the egress allowlist"} if {
    input.action == "egress"
# rule: spawn-shell-in-workspace
} else := {"allow": true, "rule": "spawn-shell-in-workspace"} if {
    input.action == "spawn"
    input.tool == "bash"
    input.target.path == data.workspace_root
# rule: spawn-mcp
} else := {"allow": true, "rule": "spawn-mcp"} if {
    input.action == "spawn"
    input.tool == "mcp"
    input.target.kind == "path"
# rule: spawn-subagent
} else := {"allow": true, "rule": "spawn-subagent"} if {
    input.action == "spawn"
    input.tool == "subagent"
# rule: spawn-job-control
} else := {"allow": true, "rule": "spawn-job-control"} if {
    input.action == "spawn"
    input.tool == "job_kill"
# rule: spawn-not-allowed
} else := {"allow": false, "rule": "spawn-not-allowed", "reason": "this process may not be spawned"} if {
    input.action == "spawn"
# rule: default-deny
} else := {"allow": false, "rule": "default-deny", "reason": "no policy rule permits this action"}

# The tools whose registration asks the capability question. The permission
# string is data, not a Rego constant, so a preset ceiling and the session mode
# are already folded into data.permission by the host.
gated_capability(tool) if {
    some gated in ["run_in_background", "job_output", "job_kill", "subagent"]
    tool == gated
}

# One comparison covers the root itself and every descendant: appending a
# slash makes the root match its own prefix, and data.workspace_prefix ends in
# "/" (a root of "/" stays "/").
inside_path(path) if {
    data.workspace_prefix != ""
    startswith(concat("", [path, "/"]), data.workspace_prefix)
}

# A path with "/.git/" or "/.bsh/" anywhere is inside those directories. The
# appended slash makes the directory itself match too.
forbidden_path(path) if {
    contains(concat("", [path, "/"]), "/.git/")
}

forbidden_path(path) if {
    contains(concat("", [path, "/"]), "/.bsh/")
}

# The unit is the resolved IP. The hostname is a hint only, so a name that
# rebinds to another address cannot ride an allowlist entry.
egress_allowed(ip) if {
    some entry in data.egress_allowlist
    entry == ip
}

egress_allowed(ip) if {
    some entry in data.egress_allowlist
    contains(entry, "/")
    net.cidr_contains(entry, ip)
}
"#;

/// The baseline as a [`PolicySource`], ready for
/// [`PolicySet::compile`](crate::policy::PolicySet::compile).
pub fn baseline_source() -> PolicySource {
    PolicySource::new("baseline", BASELINE_POLICY_REGO)
}

/// The preset path-scope module: enforced only when the preset declares a
/// read, write or deny scope. It may only narrow the baseline.
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

read_allowed(path) if {
    some root in data.preset_scope.read_roots
    path_under(path, root)
}

write_allowed(path) if {
    some root in data.preset_scope.write_roots
    path_under(path, root)
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

/// The preset egress-scope module: enforced only when the preset declares an
/// egress list. It intersects with the workspace egress allowlist.
pub const PRESET_EGRESS_REGO: &str = r#"package policy.preset_egress

# rule: preset-egress
verdict := {"allow": false, "rule": "preset-egress", "reason": "the resolved address is outside the preset's egress scope"} if {
    input.action == "egress"
    data.preset_egress.restricted
    not allowed(input.target.ip)
} else := {"allow": true}

allowed(ip) if {
    some entry in data.preset_egress.allowlist
    entry == ip
}

allowed(ip) if {
    some entry in data.preset_egress.allowlist
    contains(entry, "/")
    net.cidr_contains(entry, ip)
}
"#;

/// The preset scope module as a source.
pub fn preset_scope_source() -> PolicySource {
    PolicySource::new("preset-scope", PRESET_SCOPE_REGO)
}

/// The preset egress module as a source.
pub fn preset_egress_source() -> PolicySource {
    PolicySource::new("preset-egress", PRESET_EGRESS_REGO)
}

/// The session-permission module, generated for the EFFECTIVE level.
///
/// The session tier is a policy fact, not only a Rust registration decision: a
/// read-only session denies writes, egress and every spawn in the same
/// intersection as the baseline and the preset modules, so a tool that reaches
/// the policy directly is still refused. The level is embedded as a literal
/// because it is the resolved `min(session, preset ceiling)` value; the
/// generator normalises anything it does not know to `read-only`, so a typo
/// fails closed.
///
/// `{level}` is replaced by [`session_permission_source`]. The template itself
/// is fail-closed: a level that is neither of the two writable tiers (including
/// the unprefixed placeholder) is treated as read-only, so a module used without
/// generation denies instead of silently allowing.
pub const SESSION_PERMISSION_REGO: &str = r#"package policy.session_permission

# The session tier, generated for the effective permission "{level}".
# It can only narrow the baseline: every module must allow.
# rule: session-read-only-write
verdict := {"allow": false, "rule": "session-read-only-write", "reason": "the session's effective permission is read-only"} if {
    session_read_only
    input.action == "write"
# rule: session-read-only-egress
} else := {"allow": false, "rule": "session-read-only-egress", "reason": "the session's effective permission is read-only"} if {
    session_read_only
    input.action == "egress"
# rule: session-read-only-spawn
} else := {"allow": false, "rule": "session-read-only-spawn", "reason": "the session's effective permission is read-only"} if {
    session_read_only
    input.action == "spawn"
} else := {"allow": true}

session_permission := "{level}"

# Anything that is not one of the two writable tiers is read-only. That
# includes an unknown string and the unprefixed template, so a module used
# without generation fails closed instead of allowing everything.
session_read_only if {
    session_permission != "workspace-write"
    session_permission != "danger-full-access"
}
"#;

/// The session tier as a source, generated for `permission`.
pub fn session_permission_source(permission: &str) -> PolicySource {
    // Only the three known tiers reach the module. An unknown value becomes
    // read-only here for the same reason the resolution normalises it there:
    // a typo must not select an allowing arm.
    let level = match permission.trim() {
        "danger-full-access" => "danger-full-access",
        "workspace-write" => "workspace-write",
        _ => "read-only",
    };
    PolicySource::new(
        "session-permission",
        SESSION_PERMISSION_REGO.replace("{level}", level),
    )
}

/// The preset permission module: read-only presets deny writes even when their
/// tool list or scope is absent, so the permission ceiling is a policy fact too.
pub const PRESET_PERMISSION_REGO: &str = r#"package policy.preset_permission

# rule: preset-read-only
verdict := {"allow": false, "rule": "preset-read-only", "reason": "this preset's permission ceiling is read-only"} if {
    data.preset_permission == "read-only"
    input.action == "write"
} else := {"allow": true}
"#;

/// The preset permission module as a source.
pub fn preset_permission_source() -> PolicySource {
    PolicySource::new("preset-permission", PRESET_PERMISSION_REGO)
}
