//! The tool-boundary policy substrate.
//!
//! This module is the enforcing tier for what an agent turn may touch. It is
//! deliberately a *tool boundary*: every enforced action is a call the host
//! already makes on the model's behalf (a file read, a file write, a shell
//! spawn, a network egress), so the decision is made once, before the host
//! performs it. There is no kernel integration here and none is implied: eBPF
//! does not exist on macOS, EndpointSecurity needs an Apple entitlement, and a
//! per-syscall tier is a later slice. The seam for that tier is
//! [`PolicySet::check`]: the same [`PolicyInput`] document a syscall broker
//! would build is what a tool builds today.
//!
//! # No JIT
//!
//! Regorus has **no JIT** and generates no machine code at evaluation time.
//! The latency budget is met the only way this engine allows: compile all
//! modules once, install the data document once, and reuse the evaluator for
//! every decision in the turn. [`PolicySet::compile`] builds the interpreter
//! and, when regorus's bytecode compiler accepts a module, lowers it once into
//! an RVM program; [`PolicySet::check`] reuses the VM (the interpreter is the
//! fallback) and only installs a small input document. The input is built
//! directly as a regorus `Value` - no JSON text is serialised and parsed per
//! call. Regorus 0.13 calls the data install `Engine::add_data` rather than
//! `set_data`; it is the same compile-once/set-once contract.
//!
//! # Rule ids
//!
//! A denial may name the rule that produced it, but a Rego rule has no name the
//! host can recover from a verdict. The convention is a comment on the rule:
//!
//! ```rego
//! # rule: write-outside-workspace
//! verdict := {"allow": false, "rule": "write-outside-workspace", ...} if { ... }
//! ```
//!
//! The loader extracts every `# rule: <id>` comment into
//! [`PolicySet::rule_ids`], and the verdict's `rule` field names the one that
//! fired. A policy that denies without a `rule` field still denies; the
//! decision simply has no id to report. The baseline in
//! [`baseline`] is the reference example for both halves of the convention.
//!
//! # Composition is intersection
//!
//! Every applicable policy must allow. A source may narrow the baseline's
//! answer, never widen it: a workspace policy that says `allow` for a path the
//! baseline denies still denies, because the baseline's `verdict` is evaluated
//! too. Deny wins, and the first denying module in source order supplies the
//! reported rule and reason.
//!
//! # Failure is asymmetric
//!
//! An evaluator error, a timeout, or a missing verdict is not a reason to stop
//! the turn and is not a reason to open the door. It denies writes and network
//! egress, and allows a read that resolves inside the workspace root. That
//! asymmetry is implemented once, in [`PolicySet::check`], so a broken policy
//! can never silently widen a write or an egress.
//!
//! # The network unit is the resolved IP
//!
//! [`Target::Host`] carries both the hostname (a hint, shown to a reader) and
//! the resolved IP (the unit the policy decides on). [`EgressGate`] resolves a
//! host and checks every resolved address before a connection is made, and
//! [`PolicyResolver`] is the ureq resolver that performs the same check at the
//! exact point the HTTP client resolves a name. Deciding on the IP is what
//! defeats DNS rebinding: a name may change, the address actually connected to
//! may not.

/// One decision's audit row, the bounded denial ring and the allow counters.
pub mod audit;
/// The baseline policy shipped in-tree, and the reference example for authors.
pub mod baseline;
/// Egress resolution, the gate that enforces it, and the ureq resolver.
pub mod egress;
/// The one gated HTTP GET: a decision, then a request.
pub mod fetch;
/// Preset path scopes: validation and the glob-aware matching module.
pub mod scope;
/// Compiling sources into one engine and evaluating a decision.
pub mod set;
/// The principal, the action and the target a decision is about.
pub mod target;

pub use audit::{AllowCounts, PolicyAudit, PolicyAuditEntry, DEFAULT_AUDIT_CAPACITY};
pub use baseline::{
    BASELINE_POLICY_REGO, PRESET_EGRESS_REGO, PRESET_PERMISSION_REGO, SESSION_PERMISSION_REGO,
};
pub use scope::{is_glob, preset_scope_source, validate_scope_entry, PRESET_SCOPE_REGO};
pub use set::{
    builtin_policy_ids, PolicyData, PolicyError, PolicyModuleInfo, PolicyOrigin, PolicySet,
    PolicySource,
};

/// The shipped baseline under its stable built-in id.
pub fn baseline_source() -> PolicySource {
    baseline::baseline_source().as_builtin("builtin:baseline")
}

/// The generated session-permission module under its stable built-in id.
pub fn session_permission_source(permission: &str) -> PolicySource {
    baseline::session_permission_source(permission).as_builtin("builtin:session-permission")
}

/// The preset egress module under its stable built-in id.
pub fn preset_egress_source() -> PolicySource {
    baseline::preset_egress_source().as_builtin("builtin:preset-egress")
}

/// The generated read-only-ceiling module under its stable built-in id.
pub fn preset_permission_source() -> PolicySource {
    baseline::preset_permission_source().as_builtin("builtin:preset-permission")
}
pub use egress::{
    EgressDenial, EgressError, EgressGate, EgressRuleSet, PolicyResolver, Resolver, SystemResolver,
};
pub use fetch::{fetch, http_host, join, FetchError, FetchLimits, FetchedPage};
pub use target::{Action, Decision, PolicyInput, Target};

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_scope;
