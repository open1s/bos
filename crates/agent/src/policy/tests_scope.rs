//! Preset path-scope, built-in-id and advisory-mode tests.
//!
//! These live beside the main policy tests rather than inside them so the
//! concurrent round editing `tests.rs` cannot collide with this work.

use std::path::Path;

use super::*;

fn scratch(label: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("agent-policy-scope-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch root");
    dir
}

fn ask(tool: &str, action: Action, path: &Path) -> PolicyInput {
    PolicyInput::new(tool, action, Target::path(path))
        .with_session("session")
        .with_preset("probe")
}

/// A set with the session tier at workspace-write and the preset scope the
/// caller declares.
fn scoped(
    root: &Path,
    read: Option<Vec<String>>,
    write: Option<Vec<String>>,
    deny: Vec<String>,
) -> PolicySet {
    let read_restricted = read.is_some();
    let write_restricted = write.is_some();
    let roots = |items: Option<Vec<String>>| {
        items
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| !entry.contains('*'))
            .map(|entry| root.join(entry).to_string_lossy().to_string())
            .collect::<Vec<_>>()
    };
    let globs = |items: Option<Vec<String>>| {
        items
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| entry.contains('*'))
            .map(|entry| root.join(entry).to_string_lossy().to_string())
            .collect::<Vec<_>>()
    };
    let deny_roots = deny
        .iter()
        .filter(|entry| !entry.contains('*'))
        .map(|entry| root.join(entry).to_string_lossy().to_string())
        .collect::<Vec<_>>();
    let deny_globs = deny
        .iter()
        .filter(|entry| entry.contains('*'))
        .map(|entry| root.join(entry).to_string_lossy().to_string())
        .collect::<Vec<_>>();
    let data = PolicyData::new(root)
        .with_permission("workspace-write")
        .with_preset("probe")
        .with_preset_scope(
            read_restricted,
            roots(read.clone()),
            write_restricted,
            roots(write.clone()),
            deny_roots,
        )
        .with_preset_globs(globs(read), globs(write), deny_globs);
    PolicySet::compile(
        [
            baseline_source(),
            session_permission_source("workspace-write"),
            preset_scope_source(),
        ],
        data,
    )
}

/// A deny scope refuses a write INSIDE the workspace that the tier allows, and
/// the refusal names the preset rule.
#[test]
fn a_deny_scope_beats_the_tier_inside_the_workspace() {
    let root = scratch("deny-inside");
    let set = scoped(
        &root,
        None,
        Some(vec!["**".to_string()]),
        vec!["secrets/**".to_string()],
    );
    let denied_path = root.join("secrets").join("key.pem");
    std::fs::create_dir_all(denied_path.parent().expect("parent")).expect(".ssh");
    let decision = set.check(&ask("write_file", Action::Write, &denied_path));
    assert!(decision.is_denied(), "{decision:?}");
    assert_eq!(decision.rule.as_deref(), Some("preset-deny-path"));
    assert!(
        !denied_path.exists(),
        "the test's premise: a denied write leaves no file"
    );

    let allowed = root.join("docs").join("note.md");
    let decision = set.check(&ask("write_file", Action::Write, &allowed));
    assert!(
        decision.is_allowed(),
        "a path outside the deny glob is the tier's to allow: {decision:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A write GLOB refuses a path outside it and allows one inside, inside the
/// workspace.
#[test]
fn a_write_glob_refuses_outside_and_allows_inside() {
    let root = scratch("write-glob");
    let set = scoped(&root, None, Some(vec!["docs/**".to_string()]), Vec::new());
    assert_eq!(
        set.evaluator(),
        "bytecode",
        "a glob module must still lower to the VM, or every check pays the interpreter"
    );
    let inside = root.join("docs").join("a.md");
    let outside = root.join("src").join("a.rs");
    let decision = set.check(&ask("write_file", Action::Write, &inside));
    assert!(decision.is_allowed(), "{decision:?}");
    let decision = set.check(&ask("write_file", Action::Write, &outside));
    assert!(decision.is_denied(), "{decision:?}");
    assert_eq!(decision.rule.as_deref(), Some("preset-write-scope"));
    let _ = std::fs::remove_dir_all(&root);
}

/// A read GLOB refuses a file outside it while allowing one inside.
#[test]
fn a_read_glob_refuses_outside_and_allows_inside() {
    let root = scratch("read-glob");
    let set = scoped(
        &root,
        Some(vec!["src/**/*.rs".to_string()]),
        None,
        Vec::new(),
    );
    let inside = root.join("src").join("lib.rs");
    let outside = root.join("src").join("lib.md");
    let decision = set.check(&ask("read_file", Action::Read, &inside));
    assert!(decision.is_allowed(), "{decision:?}");
    let decision = set.check(&ask("read_file", Action::Read, &outside));
    assert!(decision.is_denied(), "{decision:?}");
    assert_eq!(decision.rule.as_deref(), Some("preset-read-scope"));
    let _ = std::fs::remove_dir_all(&root);
}

/// A literal entry keeps the prefix behaviour every shipped preset relies on:
/// the directory itself AND everything below it.
#[test]
fn a_literal_scope_keeps_the_prefix_behaviour() {
    let root = scratch("literal");
    let set = scoped(&root, Some(vec!["docs".to_string()]), None, Vec::new());
    let nested = root.join("docs").join("deep").join("note.md");
    let decision = set.check(&ask("read_file", Action::Read, &nested));
    assert!(decision.is_allowed(), "{decision:?}");
    let elsewhere = root.join("src").join("lib.rs");
    let decision = set.check(&ask("read_file", Action::Read, &elsewhere));
    assert!(decision.is_denied(), "{decision:?}");
    let _ = std::fs::remove_dir_all(&root);
}

/// Every in-binary module carries a stable `builtin:` id, its origin and its
/// trust state; the ids are exactly the reserved list.
#[test]
fn built_in_module_ids_are_listed_and_trusted() {
    let root = scratch("builtin-ids");
    let data = PolicyData::new(&root)
        .with_permission("workspace-write")
        .with_preset_scope(
            true,
            vec![root.to_string_lossy().to_string()],
            false,
            Vec::new(),
            Vec::new(),
        );
    let set = PolicySet::compile(
        [
            baseline_source(),
            session_permission_source("workspace-write"),
            preset_scope_source(),
            preset_egress_source(),
            preset_permission_source(),
        ],
        data,
    );
    assert!(set.error().is_none(), "{:?}", set.error());
    let modules = set.modules();
    let ids: Vec<&str> = modules.iter().map(|module| module.id.as_str()).collect();
    for expected in [
        "builtin:baseline",
        "builtin:session-permission",
        "builtin:preset-scope",
        "builtin:preset-egress",
        "builtin:preset-permission",
    ] {
        assert!(ids.contains(&expected), "missing {expected}: {ids:?}");
        assert!(
            builtin_policy_ids().contains(&expected),
            "{expected} is one of the documented built-in ids"
        );
    }
    for module in &modules {
        assert_eq!(module.origin, PolicyOrigin::Builtin, "{module:?}");
        assert!(module.trusted, "a built-in module is trusted: {module:?}");
        assert!(
            !module.rule_ids.is_empty(),
            "a module's rules are listed: {module:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// A workspace module that claims a `builtin:` id is a LOAD error naming both
/// the claim and the built-in it would shadow.
#[test]
fn a_user_module_cannot_shadow_a_builtin_id() {
    let claim = "# module: builtin:baseline\npackage policy.user\n\n# rule: user-allow\nverdict := {\"allow\": true, \"rule\": \"user-allow\"}\n";
    let set = PolicySet::compile(
        [
            baseline_source(),
            PolicySource::workspace("workspace", claim),
        ],
        PolicyData::new(std::env::temp_dir()),
    );
    let error = set.error().expect("the claim is a load error");
    let text = error.to_string();
    assert!(text.contains("builtin:baseline"), "{text}");
    assert!(text.contains("workspace"), "{text}");
    assert!(
        matches!(error, PolicyError::BuiltinIdClaimed { .. }),
        "{error:?}"
    );
}

/// An UNMARKED extension module's refusal stops the call.
#[test]
fn an_extension_refusal_is_hard_by_default() {
    let root = scratch("hard-refusal");
    let deny = "package policy.hard\n\n# rule: hard-deny-write\nverdict := {\"allow\": false, \"rule\": \"hard-deny-write\", \"reason\": \"no writes\"} if {\n    input.action == \"write\"\n} else := {\"allow\": true}\n";
    let set = PolicySet::compile(
        [
            baseline_source(),
            session_permission_source("workspace-write"),
            PolicySource::extension("preset", deny, true),
        ],
        PolicyData::new(&root).with_permission("workspace-write"),
    );
    let decision = set.check(&ask("write_file", Action::Write, &root.join("notes.md")));
    assert!(decision.is_denied(), "{decision:?}");
    assert!(set.advisories().is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

/// An ADVISORY preset module's refusal is recorded and the call proceeds, while
/// the floor (the baseline) still refuses outside the workspace.
#[test]
fn an_advisory_refusal_is_recorded_and_the_call_proceeds() {
    let root = scratch("advisory");
    let deny = "package policy.soft\n\n# rule: soft-deny-write\nverdict := {\"allow\": false, \"rule\": \"soft-deny-write\", \"reason\": \"this preset advises against writes\"} if {\n    input.action == \"write\"\n} else := {\"allow\": true}\n";
    let set = PolicySet::compile(
        [
            baseline_source(),
            session_permission_source("workspace-write"),
            PolicySource::extension("preset", deny, true).advisory(true),
        ],
        PolicyData::new(&root).with_permission("workspace-write"),
    );
    let target = root.join("notes.md");
    let decision = set.check(&ask("write_file", Action::Write, &target));
    assert!(
        decision.is_allowed(),
        "an advisory refusal lets the call proceed: {decision:?}"
    );
    assert_eq!(
        decision.reason.as_deref(),
        Some(
            "advisory: Refused by policy: this preset advises against writes (rule soft-deny-write)."
        )
    );
    let advisories = set.advisories();
    assert_eq!(advisories.len(), 1, "{advisories:?}");
    assert!(advisories[0].advisory);
    assert_eq!(
        advisories[0].rule.as_deref(),
        Some("soft-deny-write"),
        "the audit records the refusal verbatim"
    );
    assert!(set.denials().iter().any(|entry| entry.advisory));

    // The FLOOR is not advisory: a write outside the workspace still refuses.
    let outside = std::env::temp_dir().join("agent-policy-advisory-outside.md");
    let decision = set.check(&ask("write_file", Action::Write, &outside));
    assert!(decision.is_denied(), "{decision:?}");
    let _ = std::fs::remove_dir_all(&root);
}

/// The load-time validator refuses an unparsable glob, an empty entry and a
/// parent escape, and accepts the globs the plan documents.
#[test]
fn scope_validation_refuses_what_the_matcher_cannot_honour() {
    for bad in [
        "",
        "   ",
        "docs/[unclosed",
        "src/**/../x",
        "~/secrets",
        "a**b",
    ] {
        assert!(
            validate_scope_entry(bad).is_err(),
            "{bad:?} must be refused"
        );
    }
    for good in [
        ".",
        "docs",
        "README.md",
        "src/**/*.rs",
        "/var/log/app",
        "src/**",
        "*.md",
    ] {
        assert!(
            validate_scope_entry(good).is_ok(),
            "{good:?} must be accepted"
        );
    }
}
