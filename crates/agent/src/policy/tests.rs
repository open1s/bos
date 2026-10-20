//! Tests for the tool-boundary policy substrate.
//!
//! The acceptance criterion is proof, not a message: a denied write proves the
//! file was never created, a denied egress proves no socket was opened, and
//! every assertion is paired with a positive case so a policy that denies
//! everything cannot pass.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use super::*;

fn workspace() -> tempfile::TempDir {
    tempfile::tempdir().expect("a scratch workspace")
}

fn baseline_set(root: &Path) -> PolicySet {
    PolicySet::compile_baseline(PolicyData::new(root))
}

fn ask(tool: &str, action: Action, path: &Path) -> PolicyInput {
    PolicyInput::new(tool, action, Target::path(path))
        .with_session("session")
        .with_preset("preset")
}

#[test]
fn the_baseline_compiles_and_registers_its_rule_ids() {
    let dir = workspace();
    let set = baseline_set(dir.path());
    assert!(set.error().is_none(), "{:?}", set.error());
    let ids = set.rule_ids();
    for expected in [
        "read-inside-workspace",
        "read-outside-workspace",
        "write-inside-workspace",
        "write-outside-workspace",
        "write-forbidden-directory",
        "egress-allowlisted",
        "egress-not-allowlisted",
        "spawn-shell-in-workspace",
        "spawn-subagent",
        "spawn-job-control",
        "spawn-not-allowed",
        "capability-granted",
        "capability-read-only",
        "capability-unknown",
        "default-deny",
    ] {
        assert!(
            ids.iter().any(|id| id == expected),
            "missing {expected}: {ids:?}"
        );
    }
}

/// The generated session tier: a read-only effective permission denies write,
/// egress and spawn in Rego, and a writable one does not. The module is
/// generated from the level, so the test reads the source rather than assuming
/// a constant.
#[test]
fn the_session_permission_module_follows_the_effective_level() {
    let dir = workspace();
    let inside = dir.path().join("notes.txt");
    let read_only = session_permission_source("read-only");
    assert!(
        read_only
            .rego
            .contains("session_permission := \"read-only\""),
        "the level is embedded in the module: {}",
        read_only.rego
    );
    let set = PolicySet::compile(
        [baseline_source(), read_only],
        PolicyData::new(dir.path())
            .with_permission("read-only")
            // Allowlisted so the BASELINE allows the egress and the session
            // module is the tier that refuses it; without this the baseline's
            // own not-allowlisted rule would answer first.
            .with_egress_allowlist(["1.2.3.4"]),
    );
    assert!(set.error().is_none(), "{:?}", set.error());
    let write = set.check(&ask("write_file", Action::Write, &inside));
    assert!(!write.is_allowed(), "{write:?}");
    assert_eq!(write.rule.as_deref(), Some("session-read-only-write"));
    let egress = set.check(&PolicyInput::new(
        "mcp",
        Action::Egress,
        Target::host("example.test", "1.2.3.4"),
    ));
    assert!(!egress.is_allowed(), "{egress:?}");
    assert_eq!(egress.rule.as_deref(), Some("session-read-only-egress"));
    let spawn = set.check(&ask("bash", Action::Spawn, dir.path()));
    assert!(!spawn.is_allowed(), "{spawn:?}");
    assert_eq!(spawn.rule.as_deref(), Some("session-read-only-spawn"));

    // A writable level allows the same write inside the workspace; the read
    // side is never restricted by this module.
    let writable = PolicySet::compile(
        [
            baseline_source(),
            session_permission_source("workspace-write"),
        ],
        PolicyData::new(dir.path()).with_permission("workspace-write"),
    );
    assert!(
        writable
            .check(&ask("write_file", Action::Write, &inside))
            .is_allowed(),
        "workspace-write keeps the baseline's in-workspace write"
    );
    // An unknown level fails closed: it is generated as read-only.
    assert!(session_permission_source("typo")
        .rego
        .contains("session_permission := \"read-only\""));
    // The raw template fails closed too: a consumer that forgets to run the
    // generator gets the read-only arm, not an allow-all module.
    let raw = PolicySet::compile(
        [
            baseline_source(),
            PolicySource::new("session-permission", SESSION_PERMISSION_REGO),
        ],
        PolicyData::new(dir.path()).with_permission("read-only"),
    );
    assert!(
        !raw.check(&ask("write_file", Action::Write, &inside))
            .is_allowed(),
        "the unprefixed template must deny writes"
    );
}

#[test]
fn the_baseline_uses_the_bytecode_vm() {
    let dir = workspace();
    let set = baseline_set(dir.path());
    assert_eq!(
        set.evaluator(),
        "bytecode",
        "the baseline must lower to a reusable RVM program (error: {:?})",
        set.error()
    );
}

#[test]
fn the_bytecode_vm_and_the_interpreter_agree() {
    let dir = workspace();
    let set = PolicySet::compile_baseline(
        PolicyData::new(dir.path()).with_egress_allowlist(["127.0.0.1"]),
    );
    assert_eq!(set.evaluator(), "bytecode");
    let inside = dir.path().join("src").join("main.rs");
    let outside = dir.path().parent().expect("parent").join("outside.txt");
    let git = dir.path().join(".git").join("config");
    let inputs = [
        ask("read_file", Action::Read, &inside),
        ask("read_file", Action::Read, &outside),
        ask("write_file", Action::Write, &inside),
        ask("write_file", Action::Write, &git),
        ask("write_file", Action::Write, &outside),
        ask("bash", Action::Spawn, dir.path()),
        ask("bash", Action::Spawn, &dir.path().join("sub")),
        PolicyInput::new(
            "mcp",
            Action::Egress,
            Target::host("localhost", "127.0.0.1"),
        ),
        PolicyInput::new(
            "mcp",
            Action::Egress,
            Target::host("localhost", "203.0.113.9"),
        ),
    ];
    for input in inputs {
        let vm = set.check(&input);
        let interpreter = set.check_with_interpreter(&input);
        assert_eq!(vm, interpreter, "backends disagree on {input:?}");
    }
}

#[test]
fn reads_are_allowed_inside_and_refused_outside() {
    let dir = workspace();
    let set = baseline_set(dir.path());
    let inside = dir.path().join("notes.txt");

    let allowed = set.check(&ask("read_file", Action::Read, &inside));
    assert!(allowed.is_allowed(), "{allowed:?}");
    assert_eq!(allowed.rule.as_deref(), Some("read-inside-workspace"));

    let outside = dir.path().parent().expect("parent").join("outside.txt");
    let denied = set.check(&ask("read_file", Action::Read, &outside));
    assert!(denied.is_denied(), "{denied:?}");
    assert_eq!(denied.rule.as_deref(), Some("read-outside-workspace"));
}

#[test]
fn an_empty_workspace_root_has_no_inside() {
    // A build with no workspace must not turn every absolute path into an
    // in-workspace path through the empty prefix.
    let set = PolicySet::compile_baseline(PolicyData::new(""));
    let path = std::path::Path::new("/tmp/anything");
    assert!(set.check(&ask("read_file", Action::Read, path)).is_denied());
    assert!(set
        .check(&ask("write_file", Action::Write, path))
        .is_denied());
}

#[test]
fn a_parent_component_cannot_walk_out_of_the_workspace() {
    let dir = workspace();
    let set = baseline_set(dir.path());
    // PolicyInput normalises lexically, so /ws/sub/../../outside is /outside.
    let walked = dir
        .path()
        .join("sub")
        .join("..")
        .join("..")
        .join("outside.txt");
    let decision = set.check(&ask("write_file", Action::Write, &walked));
    assert!(decision.is_denied(), "{decision:?}");
    assert_eq!(decision.rule.as_deref(), Some("write-outside-workspace"));
}

#[test]
fn writes_are_allowed_inside_but_never_into_bsh_or_git_internals() {
    let dir = workspace();
    let set = baseline_set(dir.path());

    let inside = dir.path().join("src").join("main.rs");
    let allowed = set.check(&ask("write_file", Action::Write, &inside));
    assert!(allowed.is_allowed(), "{allowed:?}");
    assert_eq!(allowed.rule.as_deref(), Some("write-inside-workspace"));

    for forbidden in [".bsh/policy.rego", ".git/config", "src/.git/index"] {
        let path = dir.path().join(forbidden);
        let denied = set.check(&ask("write_file", Action::Write, &path));
        assert!(
            denied.is_denied(),
            "{forbidden} must be refused: {denied:?}"
        );
        assert_eq!(
            denied.rule.as_deref(),
            Some("write-forbidden-directory"),
            "{forbidden}"
        );
    }
}

#[test]
fn spawn_is_the_shell_in_the_workspace_and_nothing_else() {
    let dir = workspace();
    let set = baseline_set(dir.path());

    let allowed = set.check(&ask("bash", Action::Spawn, dir.path()));
    assert!(allowed.is_allowed(), "{allowed:?}");
    assert_eq!(allowed.rule.as_deref(), Some("spawn-shell-in-workspace"));

    let elsewhere = set.check(&ask("bash", Action::Spawn, &dir.path().join("sub")));
    assert!(elsewhere.is_denied(), "{elsewhere:?}");
    assert_eq!(elsewhere.rule.as_deref(), Some("spawn-not-allowed"));

    let other_tool = set.check(&ask("python", Action::Spawn, dir.path()));
    assert!(other_tool.is_denied(), "{other_tool:?}");
}

#[test]
fn delegation_and_job_control_are_spawns_the_baseline_permits() {
    let dir = workspace();
    let set = baseline_set(dir.path());

    // A child agent is a spawn the baseline permits; a workspace policy may
    // narrow it.
    let subagent = set.check(&ask("subagent", Action::Spawn, dir.path()));
    assert!(subagent.is_allowed(), "{subagent:?}");
    assert_eq!(subagent.rule.as_deref(), Some("spawn-subagent"));

    // Stopping a job this session owns is control, not a new process, so the
    // target's path does not have to be the workspace root.
    let kill = set.check(&ask(
        "job_kill",
        Action::Spawn,
        &dir.path().parent().expect("parent").join("elsewhere"),
    ));
    assert!(kill.is_allowed(), "{kill:?}");
    assert_eq!(kill.rule.as_deref(), Some("spawn-job-control"));

    // The shell still needs the workspace root, so this did not widen bash.
    let bash_elsewhere = set.check(&ask("bash", Action::Spawn, &dir.path().join("sub")));
    assert_eq!(bash_elsewhere.rule.as_deref(), Some("spawn-not-allowed"));
}

#[test]
fn capability_follows_the_effective_permission_in_data() {
    let dir = workspace();
    let tool_set = |permission: &str| {
        PolicySet::compile_baseline(
            PolicyData::new(dir.path()).with_permission(permission.to_string()),
        )
    };
    let gated = ["run_in_background", "job_output", "job_kill", "subagent"];

    let writable = tool_set("workspace-write");
    for tool in gated {
        let decision = writable.check(&ask(tool, Action::Capability, dir.path()));
        assert!(decision.is_allowed(), "{tool}: {decision:?}");
        assert_eq!(decision.rule.as_deref(), Some("capability-granted"));
    }

    let read_only = tool_set("read-only");
    for tool in gated {
        let decision = read_only.check(&ask(tool, Action::Capability, dir.path()));
        assert!(decision.is_denied(), "{tool}: {decision:?}");
        assert_eq!(decision.rule.as_deref(), Some("capability-read-only"));
    }

    // A name outside the gated vocabulary is not silently granted.
    let unknown = writable.check(&ask("bash", Action::Capability, dir.path()));
    assert!(unknown.is_denied(), "{unknown:?}");
    assert_eq!(unknown.rule.as_deref(), Some("capability-unknown"));
}

#[test]
fn egress_decides_on_the_resolved_address() {
    let dir = workspace();
    let set = PolicySet::compile_baseline(
        PolicyData::new(dir.path()).with_egress_allowlist(["127.0.0.1", "10.0.0.0/8"]),
    );

    let allowed = set.check(&PolicyInput::new(
        "mcp",
        Action::Egress,
        Target::host("localhost", "127.0.0.1"),
    ));
    assert!(allowed.is_allowed(), "{allowed:?}");
    assert_eq!(allowed.rule.as_deref(), Some("egress-allowlisted"));

    let cidr = set.check(&PolicyInput::new(
        "mcp",
        Action::Egress,
        Target::host("internal", "10.1.2.3"),
    ));
    assert!(cidr.is_allowed(), "{cidr:?}");

    // The hostname is a different name for the SAME allowed address, so it is
    // allowed; a different address behind the same name is not.
    let rebinding = set.check(&PolicyInput::new(
        "mcp",
        Action::Egress,
        Target::host("localhost", "203.0.113.9"),
    ));
    assert!(rebinding.is_denied(), "{rebinding:?}");
    assert_eq!(rebinding.rule.as_deref(), Some("egress-not-allowlisted"));

    // Empty allowlist is the shipped default and denies.
    let empty = baseline_set(dir.path()).check(&PolicyInput::new(
        "mcp",
        Action::Egress,
        Target::host("localhost", "127.0.0.1"),
    ));
    assert!(empty.is_denied(), "{empty:?}");
}

#[test]
fn a_preset_may_narrow_but_never_widen_the_baseline() {
    let dir = workspace();
    let widening = PolicySource::new(
        "preset",
        r#"package policy.preset

# rule: preset-allow-all
verdict := {"allow": true, "rule": "preset-allow-all"}
"#,
    );
    let set = PolicySet::compile(
        [super::baseline::baseline_source(), widening.clone()],
        PolicyData::new(dir.path()),
    );
    assert!(set.error().is_none(), "{:?}", set.error());

    // The preset says allow, the baseline says no: intersection denies.
    let outside = dir.path().parent().expect("parent").join("escaped.txt");
    let denied = set.check(&ask("write_file", Action::Write, &outside));
    assert!(denied.is_denied(), "{denied:?}");
    assert_eq!(denied.rule.as_deref(), Some("write-outside-workspace"));

    // A preset can narrow an in-workspace read the baseline allows.
    let narrowing = PolicySource::new(
        "preset",
        r#"package policy.preset

# rule: preset-deny-secrets
verdict := {"allow": false, "rule": "preset-deny-secrets", "reason": "secrets are not readable"} if {
    contains(input.target.path, "secrets")
} else := {"allow": true, "rule": "preset-allow"}
"#,
    );
    let set = PolicySet::compile(
        [super::baseline::baseline_source(), narrowing],
        PolicyData::new(dir.path()),
    );
    assert!(set.error().is_none(), "{:?}", set.error());
    let secret = dir.path().join("secrets.txt");
    let narrowed = set.check(&ask("read_file", Action::Read, &secret));
    assert!(narrowed.is_denied(), "{narrowed:?}");
    assert_eq!(narrowed.rule.as_deref(), Some("preset-deny-secrets"));
    let ordinary = dir.path().join("notes.txt");
    assert!(set
        .check(&ask("read_file", Action::Read, &ordinary))
        .is_allowed());
}

#[test]
fn a_broken_source_denies_writes_and_allows_in_workspace_reads() {
    let dir = workspace();
    let broken = PolicySource::new(
        "workspace",
        "package policy.workspace\nverdict := this is not rego",
    );
    let set = PolicySet::compile(
        [super::baseline::baseline_source(), broken],
        PolicyData::new(dir.path()),
    );
    assert!(set.error().is_some(), "the set is in its failure mode");

    let write = set.check(&ask(
        "write_file",
        Action::Write,
        &dir.path().join("new.txt"),
    ));
    assert!(write.is_denied(), "{write:?}");
    assert_eq!(write.rule.as_deref(), Some("policy-evaluator-failure"));

    let read = set.check(&ask(
        "read_file",
        Action::Read,
        &dir.path().join("notes.txt"),
    ));
    assert!(read.is_allowed(), "{read:?}");
    assert_eq!(read.rule.as_deref(), Some("policy-evaluator-failure-read"));

    let outside_read = set.check(&ask(
        "read_file",
        Action::Read,
        &dir.path().parent().expect("parent").join("outside.txt"),
    ));
    assert!(outside_read.is_denied(), "{outside_read:?}");

    let egress = set.check(&PolicyInput::new(
        "mcp",
        Action::Egress,
        Target::host("localhost", "127.0.0.1"),
    ));
    assert!(egress.is_denied(), "{egress:?}");
}

#[test]
fn a_denial_is_recorded_with_its_rule_and_allows_only_move_counters() {
    let dir = workspace();
    let set = baseline_set(dir.path()).with_audit_capacity(2);
    let outside = dir.path().parent().expect("parent").join("denied.txt");

    for _ in 0..3 {
        let decision = set.check(&ask("write_file", Action::Write, &outside));
        assert!(decision.is_denied());
    }
    let inside = dir.path().join("allowed.txt");
    for _ in 0..3 {
        assert!(set
            .check(&ask("read_file", Action::Read, &inside))
            .is_allowed());
    }

    let denials = set.denials();
    assert_eq!(denials.len(), 2, "the ring is bounded at its capacity");
    assert_eq!(denials[1].rule.as_deref(), Some("write-outside-workspace"));
    assert_eq!(denials[1].tool, "write_file");
    assert_eq!(denials[1].action, Action::Write);
    assert!(denials[1].target.contains("denied.txt"));
    assert_eq!(denials[1].session, "session");
    assert_eq!(denials[1].preset, "preset");
    assert!(denials[1].at_ms > 0);
    assert_eq!(set.denials_total(), 3, "dropped denials are still counted");

    let allows = set.allows();
    assert_eq!(allows.total, 3);
    assert_eq!(allows.reads, 3);
    assert_eq!(allows.writes, 0);
}

#[test]
fn the_refusal_names_the_rule_and_offers_no_workaround() {
    let dir = workspace();
    let set = baseline_set(dir.path());
    let outside = dir.path().parent().expect("parent").join("x.txt");
    let decision = set.check(&ask("write_file", Action::Write, &outside));
    let en = set.refusal(&decision);
    assert!(en.contains("write-outside-workspace"), "{en}");
    assert!(en.contains("Refused by policy"), "{en}");
    for hint in ["allowlist", "full access", "permission", "try"] {
        assert!(
            !en.to_ascii_lowercase().contains(hint),
            "the refusal hints at a workaround: {en}"
        );
    }
    let zh = decision.refusal("zh");
    assert!(zh.contains("策略拒绝"), "{zh}");
    assert!(zh.contains("write-outside-workspace"), "{zh}");
}

// --- egress probes -------------------------------------------------------

/// A resolver with fixed answers, so no test touches real DNS.
#[derive(Debug, Default)]
struct FakeResolver {
    answers: HashMap<String, Vec<SocketAddr>>,
    calls: AtomicUsize,
}

impl FakeResolver {
    fn with(mut self, host: &str, addr: SocketAddr) -> Self {
        self.answers.insert(host.to_string(), vec![addr]);
        self
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }
}

impl Resolver for FakeResolver {
    fn resolve(&self, host: &str, _port: u16) -> std::io::Result<Vec<SocketAddr>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.answers
            .get(host)
            .cloned()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no answer"))
    }
}

fn gate_with(policy: PolicySet, resolver: Arc<dyn Resolver>) -> Arc<EgressGate> {
    Arc::new(EgressGate::new(
        Arc::new(policy),
        resolver,
        "session",
        "preset",
        "mcp",
    ))
}

#[test]
fn a_denied_egress_never_connects() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    listener.set_nonblocking(true).expect("nonblocking");
    let port = listener.local_addr().expect("addr").port();
    let dir = workspace();
    let gate = gate_with(baseline_set(dir.path()), Arc::new(SystemResolver));

    let error = gate
        .connect("127.0.0.1", port)
        .expect_err("the empty allowlist denies loopback");
    assert!(matches!(error, EgressError::Denied(_)), "{error}");
    match listener.accept() {
        Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock),
        Ok(_) => panic!("the denied egress opened a socket"),
    }
}

#[test]
fn an_allowlisted_address_is_connected_and_receives_bytes() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("addr");
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut buffer = [0u8; 5];
        stream.read_exact(&mut buffer).expect("read");
        buffer
    });
    let dir = workspace();
    let policy = PolicySet::compile_baseline(
        PolicyData::new(dir.path()).with_egress_allowlist(["127.0.0.1"]),
    );
    let gate = gate_with(policy, Arc::new(SystemResolver));
    let mut stream = gate.connect("127.0.0.1", address.port()).expect("connect");
    stream.write_all(b"probe").expect("write");
    drop(stream);
    assert_eq!(&handle.join().expect("server"), b"probe");
}

#[test]
fn a_name_resolving_to_a_non_allowlisted_address_is_denied() {
    let dir = workspace();
    let policy = PolicySet::compile_baseline(
        PolicyData::new(dir.path()).with_egress_allowlist(["127.0.0.1"]),
    );
    let resolver = FakeResolver::default()
        .with("allowed.test", "127.0.0.1:9".parse().expect("addr"))
        .with("evil.test", "203.0.113.9:9".parse().expect("addr"));
    let gate = gate_with(policy, Arc::new(resolver));

    let allowed = gate.authorize("allowed.test", 9).expect("allowed");
    assert_eq!(allowed[0].ip().to_string(), "127.0.0.1");

    let denial = gate.authorize("evil.test", 9).expect_err("denied");
    assert_eq!(
        denial.decision.rule.as_deref(),
        Some("egress-not-allowlisted")
    );
    assert_eq!(denial.ip.as_deref(), Some("203.0.113.9"));
    assert_eq!(denial.host, "evil.test");
}

#[test]
fn resolution_is_cached_for_the_gate_lifetime() {
    let dir = workspace();
    let policy = PolicySet::compile_baseline(
        PolicyData::new(dir.path()).with_egress_allowlist(["127.0.0.1"]),
    );
    let resolver = Arc::new(
        FakeResolver::default().with("allowed.test", "127.0.0.1:9".parse().expect("addr")),
    );
    let gate = EgressGate::new(
        Arc::new(policy),
        resolver.clone(),
        "session",
        "preset",
        "mcp",
    );
    for _ in 0..5 {
        gate.authorize("allowed.test", 9).expect("allowed");
    }
    assert_eq!(resolver.calls(), 1, "one resolution serves the gate");
}

#[test]
fn a_thousand_checks_stay_under_the_ceiling() {
    let dir = workspace();
    let set = baseline_set(dir.path());
    let inside = dir.path().join("src").join("main.rs");
    let input = ask("write_file", Action::Write, &inside);

    assert_eq!(set.evaluator(), "bytecode");
    // One warm-up eval prepares the VM's rule cache, which is part of
    // compile-once rather than of the steady-state budget.
    assert!(set.check(&input).is_allowed());

    // Five batches, and the ceiling is checked against the best batch: test
    // threads run in parallel, so a single wall-clock average can double under
    // someone else's CPU load. The overall average is printed as well.
    let mut best = std::time::Duration::MAX;
    let mut total = std::time::Duration::ZERO;
    for _ in 0..5 {
        let started = Instant::now();
        for _ in 0..200 {
            std::hint::black_box(set.check(&input));
        }
        let elapsed = started.elapsed();
        total += elapsed;
        best = best.min(elapsed / 200);
    }
    let average = total / 1000;
    println!("policy check average: {average:?} (best batch {best:?})");
    assert!(
        best < std::time::Duration::from_micros(50),
        "the ceiling exists to catch a per-call compile, best batch {best:?}, average {average:?}"
    );
}
