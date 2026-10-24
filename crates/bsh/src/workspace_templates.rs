//! The authoring TEMPLATES every workspace carries in .bsh/.
//!
//! Three inert files, written beside the files they describe:
//!
//! - .bsh/policy.rego.tpl - a complete, valid workspace Rego policy: the
//!   input/data vocabulary, the rule-id convention, and five worked refusals
//!   (path, egress port, egress host, tool, unconfined spawn) behind one
//!   switch.
//! - .bsh/egress-allowlist.tpl - the Policy pane's own allowlist template,
//!   with the tri-state explained and every rule shape shown.
//! - .bsh/terminal-policy.rego.tpl - the interactive terminal gate's policy:
//!   the three actions, the answer shapes, the absence/broken tri-state, and
//!   three worked refusals (destructive input, secret redaction, shell start)
//!   behind one switch.
//!
//! The ENFORCING names are .bsh/policy.rego, .bsh/egress-allowlist and
//! .bsh/terminal-policy.rego; bsh reads those exact paths, so a .tpl file is
//! never parsed and never enforced. Copying a template means removing the .tpl
//! suffix, which is the reader's deliberate act - this module never does it.
//!
//! None of the enforcing files is seeded here. An allowlist file that exists
//! declares a floor (no rule in it is a deny-all), so auto-writing one would
//! lock a fresh workspace's network down before its owner asked; the tool policy
//! is the reader's own decision; and the terminal gate's enforcing file is the
//! pane's own to write when it actually opens. Only the templates are written,
//! only when missing, and never over a reader's edits.

use std::path::Path;

/// The policy template's file name, beside the .rego bsh enforces.
pub(crate) const POLICY_TEMPLATE_NAME: &str = "policy.rego.tpl";

/// The allowlist template's file name, beside the file bsh enforces.
pub(crate) const ALLOWLIST_TEMPLATE_NAME: &str = "egress-allowlist.tpl";

/// The terminal policy template's file name, beside the .rego the pane enforces.
pub(crate) const TERMINAL_TEMPLATE_NAME: &str = "terminal-policy.rego.tpl";

/// The directory the three files live in, relative to a workspace root.
///
/// The same .bsh directory holds the enforcing policy and allowlist, so the
/// template and the file it is copied to are always siblings.
const BSH_DIR: &str = ".bsh";

/// The complete workspace policy template.
///
/// It must stay a VALID module as written: a reader who copies it without
/// editing a single character must get a policy that parses, allows, and
/// changes nothing. The worked examples are guarded by `examples := false` and
/// are covered by tests that flip it, so the examples cannot rot into syntax
/// the engine rejects.
pub(crate) const POLICY_TEMPLATE: &str = r##"# =============================================================================
# Workspace tool policy - TEMPLATE (not enforced)
#
# bsh reads .bsh/policy.rego, NOT this file. Copy this file to
# .bsh/policy.rego (same folder, drop the .tpl suffix) to make it this
# workspace's policy. Every read, write, egress and spawn then asks this module
# as well as the baseline, the session tier and the preset modules.
#
# A workspace policy can only NARROW: every module must allow, so nothing here
# can widen a refusal another module already made. The verdict below therefore
# ALLOWS by default; the worked examples refuse only when switched on.
#
# If this file does not parse, or its verdict is undefined for a question, the
# host applies its failure asymmetry: writes and egress are DENIED and a read
# that resolves inside the workspace is allowed. A broken policy never opens
# the door.
#
# The input document, one question at a time:
#   input.session         the turn's session id
#   input.preset          the agent preset id in effect ("" when none)
#   input.tool            the tool asking: read_file, write_file, bash,
#                         browser_open, mcp, run_in_background, subagent, ...
#   input.action          "read" | "write" | "egress" | "spawn" | "capability"
#   input.target.kind     "path" | "host"
#   input.target.path     absolute path (kind == "path")
#   input.target.host     hostname as written (kind == "host")
#   input.target.ip       the resolved address the decision is made on
#   input.target.port     destination port, or null when the caller had none
#   input.workspace_root  the absolute workspace root
#
# The data document, installed once per build:
#   data.workspace_root / data.workspace_prefix   the root, and "root/" for a
#                                                 single startswith containment
#   data.permission                               effective mode: "read-only",
#                                                 "workspace-write" or
#                                                 "danger-full-access"
#   data.preset / data.preset_permission          preset in force, its ceiling
#   data.sandbox_enabled                          the settings master switch:
#                                                 true = the switch is on, false
#                                                 = it is off for every preset.
#   data.sandbox_confined                         the RESOLVED answer: true only
#                                                 while heel actually confines
#                                                 this turn. False with the switch
#                                                 off AND at full access (where
#                                                 the permission releases the
#                                                 shell, unless a preset forces
#                                                 heel). A policy that REQUIRES
#                                                 confinement reads THIS key; the
#                                                 spawn question is still asked,
#                                                 so it can refuse (example 5).
#   data.egress_unrestricted                      true when the workspace has NO
#                                                 .bsh/egress-allowlist file
#   data.egress_allowlist / data.egress_hosts / data.egress_ports
#                                                 the workspace allowlist rules
#   data.preset_egress_allowlist / _hosts / _ports
#                                                 the preset's own egress rules
#
# Rule ids: a "# rule: <id>" comment is reported with a refusal; the verdict's
# "rule" field repeats the id. A denial's "reason" is shown to the reader, so
# write it for a person. The Policy pane lists this module and its rule ids.
# =============================================================================

package policy.workspace

# -----------------------------------------------------------------------------
# The switch. The worked examples are compiled but inert while this is false,
# so copying this template changes nothing. Set it to true after editing the
# lists below.
# -----------------------------------------------------------------------------
examples := false

# -----------------------------------------------------------------------------
# The verdict. The first matching branch decides; the last branch allows.
# -----------------------------------------------------------------------------

# rule: workspace-deny-path
verdict := {"allow": false, "rule": "workspace-deny-path", "reason": "the path is on this workspace's deny list"} if {
    examples
    denied_path(input.target)
# rule: workspace-deny-egress-port
} else := {"allow": false, "rule": "workspace-deny-egress-port", "reason": "the destination port is not on this workspace's list"} if {
    examples
    denied_port(input)
# rule: workspace-deny-host
} else := {"allow": false, "rule": "workspace-deny-host", "reason": "the destination is on this workspace's deny list"} if {
    examples
    denied_host(input)
# rule: workspace-deny-tool
} else := {"allow": false, "rule": "workspace-deny-tool", "reason": "this workspace does not offer this tool"} if {
    examples
    denied_tool(input)
# rule: workspace-require-sandbox
} else := {"allow": false, "rule": "workspace-require-sandbox", "reason": "this workspace requires the sandbox; turn it back on to run commands"} if {
    examples
    denied_unconfined_spawn(input)
# rule: workspace-allow
} else := {"allow": true, "rule": "workspace-allow"}

# -----------------------------------------------------------------------------
# 1. Paths this workspace refuses.
#
# deny_paths holds absolute prefixes: the directory itself and everything under
# it. The trailing slash is required.
#
# deny_segments holds names that are refused wherever they appear, so ".env"
# also refuses "config/.env".
# -----------------------------------------------------------------------------
deny_paths := [
    # "/absolute/path/to/your-secrets/",
]

deny_segments := [
    # ".env",
    # "id_rsa",
]

denied_path(target) if {
    target.kind == "path"
    some prefix in deny_paths
    startswith(concat("", [target.path, "/"]), prefix)
}

# The appended slash makes a file with that exact name match as a segment.
denied_path(target) if {
    target.kind == "path"
    some segment in deny_segments
    contains(concat("", [target.path, "/"]), concat("", ["/", segment, "/"]))
}

# -----------------------------------------------------------------------------
# 2. Destination ports (egress only).
#
# Flip enforce_egress_ports to true to use the list. An empty list with the
# switch on refuses EVERY port, and a request whose port the caller could not
# state is refused too: a port list never admits an unknown port.
# -----------------------------------------------------------------------------
enforce_egress_ports := false

allowed_egress_ports := [
    # 443,
]

denied_port(input) if {
    enforce_egress_ports
    input.action == "egress"
    not port_allowed(input.target.port)
}

port_allowed(port) if {
    some allowed in allowed_egress_ports
    port == allowed
}

# -----------------------------------------------------------------------------
# 3. Destination hosts (egress only).
#
# The allowlist file is the normal door. This is the other direction: refuse a
# destination the allowlist would otherwise permit. Matching the HOSTNAME is
# the weaker door (a DNS rebind keeps the name); the allowlist matches the
# resolved address for exactly that reason. Suffixes start with "." and match
# any name ending in them.
# -----------------------------------------------------------------------------
deny_hosts := [
    # "telemetry.example.com",
]

deny_host_suffixes := [
    # ".ads.example.com",
]

denied_host(input) if {
    input.action == "egress"
    some host in deny_hosts
    input.target.host == host
}

denied_host(input) if {
    input.action == "egress"
    some suffix in deny_host_suffixes
    endswith(input.target.host, suffix)
}

# -----------------------------------------------------------------------------
# 4. Tools this workspace does not offer.
#
# The tool name refuses both the registration question ("capability", asked for
# the gated tools bash, run_in_background, job_output, job_kill and subagent)
# and every call-time action, so a tool named here cannot slip in through a
# turn that was built before this policy was saved.
# -----------------------------------------------------------------------------
deny_tools := [
    # "browser_open",
    # "mcp",
]

denied_tool(input) if {
    some tool in deny_tools
    input.tool == tool
}

# -----------------------------------------------------------------------------
# 5. Require the sandbox for shell spawns.
#
# data.sandbox_confined is the RESOLVED answer: true only while heel actually
# confines this turn. It is the key to require, because the master switch alone
# is not enough - at full access the permission releases the shell with the
# switch still on, and a preset's `sandbox = "heel"` confines a full-access
# session, which this key follows. The spawn question is asked even for an
# unconfined shell, so a workspace that REQUIRES confinement can refuse it
# here: the call fails before a process exists. Set require_sandbox to true to
# make confinement a floor rather than a release. The interactive terminal is
# a separate seam with its own .bsh/terminal-policy.rego gate.
# -----------------------------------------------------------------------------
require_sandbox := false

denied_unconfined_spawn(input) if {
    require_sandbox
    not data.sandbox_confined
    input.action == "spawn"
}
"##;

/// The header the allowlist template adds above the pane's own copy.
const ALLOWLIST_HEADER: &str = "\
# =============================================================================
# Egress allowlist - TEMPLATE (not enforced)
#
# bsh reads .bsh/egress-allowlist, NOT this file. Copy this file to
# .bsh/egress-allowlist (same folder, drop the .tpl suffix) to make it this
# workspace's allowlist. The rules below are the same copy the Policy pane's
# \"Create starter allowlist\" writes, so the two cannot describe different
# shapes.
#
# The file is TRI-STATE: no file at all leaves the network UNRESTRICTED; a file
# that exists and declares no rule DENIES ALL egress; rules narrow the reach to
# what they name. Every rule example below is commented out, so copying this
# template AS IS is the deliberate deny-all floor - uncomment a rule to open it.
# =============================================================================

";

/// The allowlist template: the copy instruction, then the pane's own rules.
///
/// The rules are NOT duplicated here: they are
/// [crate::caps::EGRESS_ALLOWLIST_TEMPLATE], the single source of truth the
/// Policy pane's Create action writes, so the file a reader copies and the file
/// the pane creates can never describe different rule shapes.
pub(crate) fn allowlist_template() -> String {
    format!(
        "{ALLOWLIST_HEADER}{}",
        crate::caps::EGRESS_ALLOWLIST_TEMPLATE
    )
}

/// The header the terminal template adds above the policy body.
///
/// It carries the three facts a terminal policy author needs and the body cannot
/// state about itself: the .tpl is not the enforced name, absence and a parse
/// failure are different states (open vs fail-closed, with the last good policy
/// kept), and this gate covers the interactive pane while .bsh/policy.rego
/// covers tool calls.
const TERMINAL_HEADER: &str = r#"# =============================================================================
# Interactive terminal policy - TEMPLATE (not enforced)
#
# bsh reads .bsh/terminal-policy.rego, NOT this file. Copy this file to
# .bsh/terminal-policy.rego (same folder, drop the .tpl suffix) to make it this
# workspace's terminal policy. The pane also writes this same policy body to
# that name when it first opens in a workspace that has none, so the file the
# pane drops and the file a reader copies carry the same rules.
#
# The file is TRI-STATE:
#
#   no file at all        every gate is OPEN. That is the documented default,
#                         and the shipped policy module is the only one that
#                         applies: nothing is refused.
#   a file that parses    decides. It can only NARROW, because the shipped
#                         allow-all baseline stays underneath it.
#   a file that does      closes ALL THREE gates - start, input and output. An
#   NOT parse             unusable rule must never widen the gate it was meant
#                         to narrow: on the first load the pane reports the
#                         refusal and spawns nothing. A broken RELOAD keeps the
#                         LAST GOOD policy in force and records the parse error
#                         in the audit ring and the status line; deleting the
#                         file restores the allow-all default.
#
# This gate is the INTERACTIVE PANE's: the shell it spawns, the line a reader
# submits, and the chunk read back from the PTY. It is NOT the workspace tool
# policy - .bsh/policy.rego, whose template is policy.rego.tpl beside this file,
# gates TOOL CALLS (read, write, egress, spawn, capability). The two are
# separate seams answering different questions, and neither widens the other.
#
# The body below documents the exact input and answer shapes and carries three
# worked examples behind one switch.
# =============================================================================

"#;

/// The terminal template: the copy instruction and the tri-state, then the
/// policy body the pane itself seeds.
///
/// The body is NOT duplicated here: it is
/// [crate::terminal::WORKSPACE_POLICY_TEMPLATE], the single source of truth
/// `Terminal::build` writes into a workspace whose gate policy is missing, so
/// the file a reader copies and the file the pane drops can never describe
/// different rules.
pub(crate) fn terminal_template() -> String {
    format!(
        "{TERMINAL_HEADER}{}",
        crate::terminal::WORKSPACE_POLICY_TEMPLATE
    )
}

/// Write all three templates into <root>/.bsh, when they are missing.
///
/// Only the .tpl files are written: policy.rego, egress-allowlist and
/// terminal-policy.rego are never seeded here, because each is the reader's own
/// decision (a present allowlist whose rules are all commented is a deny-all,
/// and the terminal pane writes its own starter when it actually opens). An
/// existing template is left byte-for-byte alone, so edits survive every launch.
///
/// A root that is not a directory is skipped rather than created: a workspace
/// record pointing at a deleted folder must not resurrect it.
pub(crate) fn ensure(root: &Path) -> std::io::Result<()> {
    if !root.is_dir() {
        return Ok(());
    }
    let dir = root.join(BSH_DIR);
    std::fs::create_dir_all(&dir)?;
    for (name, body) in [
        (POLICY_TEMPLATE_NAME, POLICY_TEMPLATE.to_string()),
        (ALLOWLIST_TEMPLATE_NAME, allowlist_template()),
        (TERMINAL_TEMPLATE_NAME, terminal_template()),
    ] {
        let path = dir.join(name);
        if !path.exists() {
            std::fs::write(path, body)?;
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use agent::policy::{Action, PolicyInput, PolicySet, PolicySource, Target};
    use std::path::PathBuf;

    /// A scratch workspace under target/, removed on the way in.
    fn scratch(label: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/bsh-workspace-templates")
            .join(format!("{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch workspace");
        dir
    }

    /// Compile the shipped baseline plus one workspace module.
    ///
    /// Egress is left UNRESTRICTED so a refusal in these tests can only come
    /// from the template: the baseline's own allowlist is not what is under test.
    fn compile(root: &Path, rego: &str) -> PolicySet {
        compile_with_sandbox(root, rego, true)
    }

    /// The same compile, with the settings switch and the resolved
    /// confinement as data.
    ///
    /// `compile_with_sandbox` keeps them consistent (switch off => released),
    /// which is the ordinary case; `compile_with` spells the full-access and
    /// preset-heel cases where the switch and the resolved answer differ.
    fn compile_with_sandbox(root: &Path, rego: &str, sandbox_enabled: bool) -> PolicySet {
        compile_with(root, rego, sandbox_enabled, sandbox_enabled)
    }

    fn compile_with(
        root: &Path,
        rego: &str,
        sandbox_enabled: bool,
        sandbox_confined: bool,
    ) -> PolicySet {
        let data = agent::policy::PolicyData::new(root.to_path_buf())
            .with_permission(crate::caps::WORKSPACE_WRITE)
            .with_egress_unrestricted(true)
            .with_sandbox_enabled(sandbox_enabled)
            .with_sandbox_confined(sandbox_confined);
        PolicySet::compile(
            [
                agent::policy::baseline_source(),
                PolicySource::workspace("workspace", rego),
            ],
            data,
        )
    }

    fn ask(tool: &str, action: Action, target: Target) -> PolicyInput {
        PolicyInput::new(tool, action, target).with_session("session")
    }

    /// Both templates land beside the files they describe, and neither ENFORCING
    /// file is created: the allowlist would deny all egress, the policy would
    /// decide for the reader.
    #[test]
    fn the_templates_are_written_and_nothing_is_enforced() {
        let root = scratch("write");
        ensure(&root).expect("the templates are written");
        let bsh = root.join(BSH_DIR);
        let policy = std::fs::read_to_string(bsh.join(POLICY_TEMPLATE_NAME)).expect("policy");
        let allowlist =
            std::fs::read_to_string(bsh.join(ALLOWLIST_TEMPLATE_NAME)).expect("allowlist");
        let terminal = std::fs::read_to_string(bsh.join(TERMINAL_TEMPLATE_NAME)).expect("terminal");

        assert!(
            !bsh.join("policy.rego").exists(),
            "no enforcing policy is seeded"
        );
        assert!(
            !bsh.join("egress-allowlist").exists(),
            "no enforcing allowlist is seeded: a present file would deny all egress"
        );
        assert!(
            !bsh.join("terminal-policy.rego").exists(),
            "no enforcing terminal policy is seeded: only the pane writes that name"
        );
        assert!(policy.contains("package policy.workspace"));
        assert!(policy.contains("Copy this file to\n# .bsh/policy.rego"));
        assert!(policy.contains("A workspace policy can only NARROW"));
        assert!(policy.contains("examples := false"));
        assert!(
            policy.contains("data.sandbox_enabled")
                && policy.contains("data.sandbox_confined")
                && policy.contains("not data.sandbox_confined")
                && policy.contains("require_sandbox := false"),
            "the vocabulary teaches the switch AND the resolved key, and the example is inert"
        );
        assert!(allowlist.contains(".bsh/egress-allowlist"));
        assert!(allowlist.starts_with("# ====="));
        assert!(allowlist.contains("The file is TRI-STATE"));
        assert!(allowlist.ends_with(crate::caps::EGRESS_ALLOWLIST_TEMPLATE));
        assert!(terminal.contains(".bsh/terminal-policy.rego"));
        assert!(terminal.contains("NOT this file"));
        assert!(terminal.starts_with("# ====="));
        assert!(terminal.contains("TEMPLATE (not enforced)"));
        assert!(
            terminal.ends_with(crate::terminal::WORKSPACE_POLICY_TEMPLATE),
            "the template body is the pane's own policy body, not a second copy"
        );
        assert!(
            agent::policy::EgressRuleSet::parse([allowlist.as_str()]).is_empty(),
            "copying the allowlist template as-is is the deliberate deny-all floor"
        );
    }

    /// An existing template is never overwritten: a reader edits it once and the
    /// edits survive every launch.
    #[test]
    fn an_edited_template_survives_a_later_ensure() {
        let root = scratch("keep-edits");
        ensure(&root).expect("first write");
        let policy = root.join(BSH_DIR).join(POLICY_TEMPLATE_NAME);
        let terminal = root.join(BSH_DIR).join(TERMINAL_TEMPLATE_NAME);
        std::fs::write(&policy, "# my policy\n").expect("the edit");
        std::fs::write(&terminal, "# my terminal policy\n").expect("the edit");
        ensure(&root).expect("second write");
        assert_eq!(
            std::fs::read_to_string(&policy).expect("still there"),
            "# my policy\n",
            "a template is documentation the reader may edit, so it is write-if-missing only"
        );
        assert_eq!(
            std::fs::read_to_string(&terminal).expect("still there"),
            "# my terminal policy\n",
            "the terminal template is write-if-missing on the same terms"
        );
    }

    /// A workspace record pointing at a directory that is gone must not
    /// resurrect it just to drop two files into it.
    #[test]
    fn a_missing_root_is_skipped_not_created() {
        let root = scratch("missing-root").join("deleted");
        ensure(&root).expect("a missing root is not an error");
        assert!(!root.exists(), "ensure must not create the workspace root");
    }

    /// The template as written is a VALID module that allows everything the
    /// baseline allows: copying it cannot change behaviour by accident.
    #[test]
    fn the_policy_template_compiles_and_changes_nothing() {
        let root = scratch("policy-noop");
        let set = compile(&root, POLICY_TEMPLATE);
        assert!(set.error().is_none(), "{:?}", set.error());
        for id in [
            "workspace-deny-path",
            "workspace-deny-egress-port",
            "workspace-deny-host",
            "workspace-deny-tool",
            "workspace-allow",
        ] {
            assert!(
                set.rule_ids().iter().any(|rule| rule == id),
                "{id} is not declared in the template: {:?}",
                set.rule_ids()
            );
        }
        // A path the examples name is allowed while the switch is off...
        let secret = ask(
            "read_file",
            Action::Read,
            Target::path(root.join("secrets/key.pem")),
        );
        let decision = set.check(&secret);
        assert!(
            decision.is_allowed(),
            "the examples are inert: {decision:?}"
        );
        // ...and a normal write passes both modules.
        let write = ask(
            "write_file",
            Action::Write,
            Target::path(root.join("notes.txt")),
        );
        assert!(set.check(&write).is_allowed());
    }

    /// The worked examples compile and refuse when the switch is flipped:
    /// a template whose examples do not run is a trap, not a template.
    #[test]
    fn the_worked_examples_refuse_when_switched_on() {
        // The engine normalises a target path, so the prefix injected into the
        // example list must be normalised too (the scratch root contains "..").
        let root = std::fs::canonicalize(scratch("policy-examples")).expect("an absolute root");
        let active = POLICY_TEMPLATE
            .replace("examples := false", "examples := true")
            .replace(
                "    # \"/absolute/path/to/your-secrets/\",",
                &format!("    \"{}/secrets/\",", root.display()),
            )
            .replace(
                "    # \"telemetry.example.com\",",
                "    \"telemetry.example.com\",",
            );
        assert_ne!(active, POLICY_TEMPLATE, "the anchors moved");
        let set = compile(&root, &active);
        assert!(set.error().is_none(), "{:?}", set.error());

        let denied = set.check(&ask(
            "read_file",
            Action::Read,
            Target::path(root.join("secrets/key.pem")),
        ));
        assert!(denied.is_denied(), "{denied:?}");
        assert_eq!(
            denied.rule.as_deref(),
            Some("workspace-deny-path"),
            "the refusal names the rule that fired"
        );

        let allowed = set.check(&ask(
            "read_file",
            Action::Read,
            Target::path(root.join("notes.txt")),
        ));
        assert!(allowed.is_allowed(), "{allowed:?}");

        let host = set.check(&ask(
            "browser_open",
            Action::Egress,
            Target::host("telemetry.example.com", "203.0.113.9"),
        ));
        assert!(host.is_denied(), "{host:?}");
        assert_eq!(host.rule.as_deref(), Some("workspace-deny-host"));

        let other = set.check(&ask(
            "browser_open",
            Action::Egress,
            Target::host("example.com", "203.0.113.10"),
        ));
        assert!(other.is_allowed(), "{other:?}");
    }

    /// A workspace policy can read the RESOLVED sandbox state and refuse an
    /// unconfined spawn.
    ///
    /// The worked example requires `data.sandbox_confined`, so the same module
    /// answers differently for every arm that changes confinement: allowed while
    /// heel applies, denied with the switch off, denied at full access with the
    /// switch still on (the permission released the shell), and allowed again
    /// for a preset-forced heel at full access. That is the answer to "can a
    /// workspace deny an unconfined shell": yes, the spawn question is asked.
    #[test]
    fn a_workspace_policy_can_read_the_sandbox_switch_and_refuse_a_spawn() {
        let root = std::fs::canonicalize(scratch("policy-sandbox-switch")).expect("a root");
        let active = POLICY_TEMPLATE
            .replace("examples := false", "examples := true")
            .replace("require_sandbox := false", "require_sandbox := true");
        assert_ne!(active, POLICY_TEMPLATE, "the anchors moved");

        let spawn = || ask("bash", Action::Spawn, Target::path(root.clone()));

        // Switch on, heel applies.
        let on = compile_with_sandbox(&root, &active, true);
        assert!(on.error().is_none(), "{:?}", on.error());
        let allowed = on.check(&spawn());
        assert!(
            allowed.is_allowed(),
            "with the sandbox on, the spawn is allowed: {allowed:?}"
        );

        // Switch off: released.
        let off = compile_with_sandbox(&root, &active, false);
        assert!(off.error().is_none(), "{:?}", off.error());
        let denied = off.check(&spawn());
        assert!(
            denied.is_denied(),
            "the same workspace policy refuses an unconfined spawn: {denied:?}"
        );
        assert_eq!(
            denied.rule.as_deref(),
            Some("workspace-require-sandbox"),
            "the refusal names the rule that decided it"
        );
        assert!(
            off.check(&ask(
                "read_file",
                Action::Read,
                Target::path(root.join("notes.txt")),
            ))
            .is_allowed(),
            "the rule is about spawns, not reads"
        );

        // Switch ON, but full access released the shell: the resolved key sees
        // it where the switch alone would not.
        let released = compile_with(&root, &active, true, false);
        assert!(
            released.check(&spawn()).is_denied(),
            "a full-access release is refused even with the switch on"
        );

        // Switch ON and a preset forces heel at full access: confined again, so
        // the same rule allows it.
        let forced = compile_with(&root, &active, true, true);
        assert!(
            forced.check(&spawn()).is_allowed(),
            "a preset-forced heel is confined, not refused: {:?}",
            forced.check(&spawn())
        );
    }

    /// The loader reads the exact .rego name: the .tpl beside it is ignored, and
    /// copying the template to policy.rego is what applies it. The workspace's
    /// egress state is untouched by either file.
    #[test]
    fn the_loader_ignores_the_template_until_it_is_copied() {
        let root = scratch("loader");
        ensure(&root).expect("templates");
        let mut settings = crate::settings::Settings::default();
        let id = settings.note_workspace("scratch", &root.to_string_lossy());
        settings.global.general.active_workspace = id;
        let build = || {
            let eff = settings.effective();
            crate::caps::policy_for_effective(&settings, &eff)
        };

        let before = build();
        assert!(before.error().is_none(), "{:?}", before.error());
        assert!(
            !before
                .modules()
                .iter()
                .any(|module| module.id == "workspace"),
            "a .tpl file must never be compiled"
        );

        std::fs::copy(
            root.join(BSH_DIR).join(POLICY_TEMPLATE_NAME),
            root.join(BSH_DIR).join("policy.rego"),
        )
        .expect("the reader copies the template");
        let after = build();
        assert!(after.error().is_none(), "{:?}", after.error());
        assert!(
            after
                .modules()
                .iter()
                .any(|module| module.id == "workspace"),
            "the copied file is the workspace module"
        );
        let write = after.check(&ask(
            "write_file",
            Action::Write,
            Target::path(root.join("notes.txt")),
        ));
        assert!(
            write.is_allowed(),
            "copying the template does not change what is allowed: {write:?}"
        );
    }

    /// The terminal template states the facts a policy author cannot infer from
    /// the rules alone, and it must keep naming the three actions and both
    /// answer shapes: a cheap guard against silent divergence.
    #[test]
    fn the_terminal_template_names_the_contract() {
        let root = scratch("terminal-contract");
        ensure(&root).expect("templates");
        let text = std::fs::read_to_string(root.join(BSH_DIR).join(TERMINAL_TEMPLATE_NAME))
            .expect("the terminal template");

        // The loader's package: the gate evaluates data.terminal.verdict.
        assert!(text.contains("package terminal"), "the loader's package");

        // The three decisions, by the field the gate actually sends.
        for action in ["start", "input", "output"] {
            assert!(
                text.contains(&format!("input.action == \"{action}\"")),
                "the template no longer names the {action} action"
            );
        }
        // The exact input shapes.
        for shape in [
            "\"command\": <shell path>",
            "\"shell\": <shell path>",
            "\"cwd\": <workspace>",
            "\"text\": <one chunk read from the PTY>",
            "\"deny\": false",
        ] {
            assert!(text.contains(shape), "the {shape} field is undocumented");
        }
        // The answer shapes: a boolean, or {allow, reason, redact}.
        assert!(text.contains("Answer with a BOOLEAN"));
        assert!(text.contains("{\"allow\": true}"));
        assert!(text.contains("{\"allow\": false, \"reason\": "));
        assert!(text.contains("\"redact\":"));

        // The three facts the header must carry.
        assert!(text.contains("TEMPLATE (not enforced)"));
        assert!(text.contains("bsh reads .bsh/terminal-policy.rego, NOT this file"));
        assert!(text.contains("The file is TRI-STATE"));
        assert!(
            text.contains("closes ALL THREE gates"),
            "a broken file closes every gate"
        );
        assert!(
            text.contains("LAST GOOD policy"),
            "a broken reload keeps the last good policy"
        );
        assert!(
            text.contains("INTERACTIVE PANE") && text.contains(".bsh/policy.rego"),
            "the interactive gate and the tool-call gate must be named as separate seams"
        );
        assert!(text.contains("flipping it to true on its own changes NOTHING"));
        assert!(text.contains("examples := false"));
        assert!(
            text.ends_with(crate::terminal::WORKSPACE_POLICY_TEMPLATE),
            "the body is the pane's own policy body, not a second copy"
        );
    }

    /// The template as written is a VALID module for the loader the pane uses,
    /// and copying it verbatim permits exactly what a missing file permits:
    /// the start and input decisions are compared, so "identical to today" is
    /// asserted rather than assumed.
    #[test]
    fn the_terminal_template_compiles_and_changes_nothing() {
        let root = scratch("terminal-noop");
        ensure(&root).expect("templates");
        let bsh = root.join(BSH_DIR);
        let enforcing = bsh.join("terminal-policy.rego");
        assert!(
            !enforcing.exists(),
            "ensure must never create the file the pane enforces"
        );

        let mut missing = crate::terminal::gate_for_workspace(&root);
        assert_eq!(
            missing.source(),
            crate::terminal::PolicySource::Default,
            "no file means the shipped allow-all policy"
        );
        let shell = Path::new("/bin/zsh");
        let before_start = missing.start(shell, &root);
        let before_input = missing.input("rm -rf build", &root);

        std::fs::copy(bsh.join(TERMINAL_TEMPLATE_NAME), &enforcing).expect("the reader copies it");
        let mut copied = crate::terminal::gate_for_workspace(&root);
        assert_eq!(
            copied.source(),
            crate::terminal::PolicySource::File,
            "the copy is the workspace module"
        );
        let after_start = copied.start(shell, &root);
        let after_input = copied.input("rm -rf build", &root);

        // A gate that did not parse closes every gate and refuses, so an allowed
        // start also proves the template parsed.
        assert!(
            after_start.allowed(),
            "the copied template parsed and allowed the start: {after_start:?}"
        );
        assert_eq!(
            before_start, after_start,
            "copying the template does not change a start decision"
        );
        assert_eq!(
            before_input, after_input,
            "copying the template does not change an input decision"
        );
        assert!(before_input.allowed(), "an unlisted line still runs");
        assert!(after_input.redaction().is_none(), "and nothing is redacted");
    }

    /// Flipping the switch on its own changes NOTHING, because every list it
    /// reads starts empty. With one scratch value injected per list, each worked
    /// example fires through the same loader the pane uses.
    #[test]
    fn the_terminal_examples_refuse_when_switched_on() {
        let root = scratch("terminal-examples");
        ensure(&root).expect("templates");
        let bsh = root.join(BSH_DIR);
        let template = std::fs::read_to_string(bsh.join(TERMINAL_TEMPLATE_NAME))
            .expect("the terminal template");

        // The switch alone: the examples are compiled but every list is empty.
        let switched = template.replace("examples := false", "examples := true");
        assert_ne!(switched, template, "the switch anchor moved");
        std::fs::write(bsh.join("terminal-policy.rego"), &switched).expect("the switch alone");
        let mut inert = crate::terminal::gate_for_workspace(&root);
        assert!(
            inert.start(Path::new("/bin/bash"), &root).allowed(),
            "an empty deny list refuses no shell"
        );
        assert!(
            inert.input("rm -rf build", &root).allowed(),
            "an empty command list refuses no line"
        );
        assert!(
            inert.output("AKIAIOSFODNN7EXAMPLE").redaction().is_none(),
            "an empty pattern list redacts nothing"
        );

        // One value per list: all three examples fire.
        let active = template
            .replace("examples := false", "examples := true")
            .replace("    # \"rm -rf\",", "    \"rm -rf\",")
            .replace("    # `AKIA[0-9A-Z]{16}`,", "    `AKIA[0-9A-Z]{16}`,")
            .replace("    # \"/bin/bash\",", "    \"/bin/bash\",");
        assert_ne!(active, template, "an example anchor moved");
        std::fs::write(bsh.join("terminal-policy.rego"), &active).expect("the live examples");
        let mut gate = crate::terminal::gate_for_workspace(&root);

        let destructive = gate.input("rm -rf build", &root);
        assert!(!destructive.allowed(), "{destructive:?}");
        assert_eq!(
            destructive.reason(),
            Some("this command is on the workspace's destructive list")
        );
        assert!(
            gate.input("ls -la", &root).allowed(),
            "unlisted lines still run"
        );

        let refused_start = gate.start(Path::new("/bin/bash"), &root);
        assert!(!refused_start.allowed(), "{refused_start:?}");
        assert_eq!(
            refused_start.reason(),
            Some("this shell is not used in this workspace")
        );
        assert!(gate.start(Path::new("/bin/zsh"), &root).allowed());

        let redacted = gate.output("token AKIAIOSFODNN7EXAMPLE");
        assert!(
            redacted.allowed(),
            "a redaction still lets the chunk through"
        );
        assert_eq!(
            redacted.redaction(),
            Some("[redacted]"),
            "the matched secret is replaced, not dropped"
        );
        assert_eq!(gate.output("nothing secret").redaction(), None);
    }

    /// The terminal loader reads the exact enforced name: the .tpl beside it is
    /// ignored, and copying the template is what applies it.
    #[test]
    fn the_terminal_loader_ignores_the_template_until_it_is_copied() {
        let root = scratch("terminal-loader");
        ensure(&root).expect("templates");
        let bsh = root.join(BSH_DIR);
        let template = bsh.join(TERMINAL_TEMPLATE_NAME);
        let enforcing = bsh.join("terminal-policy.rego");

        // A template that would refuse a start if anything ever compiled it.
        std::fs::write(
            &template,
            r#"package terminal

decision := {"allow": false, "reason": "copied"} if {
    input.action == "start"
}
"#,
        )
        .expect("a discriminating template");

        let mut before = crate::terminal::gate_for_workspace(&root);
        assert_eq!(
            before.source(),
            crate::terminal::PolicySource::Default,
            "a .tpl file must never be compiled"
        );
        assert!(
            before.start(Path::new("/bin/zsh"), &root).allowed(),
            "and nothing is refused while the enforced name is absent"
        );

        std::fs::copy(&template, &enforcing).expect("the reader copies it");
        let mut after = crate::terminal::gate_for_workspace(&root);
        assert_eq!(after.source(), crate::terminal::PolicySource::File);
        let denied = after.start(Path::new("/bin/zsh"), &root);
        assert!(
            !denied.allowed(),
            "the copy is what the loader applies: {denied:?}"
        );
        assert_eq!(denied.reason(), Some("copied"));
    }
}
