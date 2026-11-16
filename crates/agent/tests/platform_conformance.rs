//! Cross-platform tool execution conformance (issue #313).
//!
//! Every platform-sensitive tool runs through the shared hostkit process
//! runtime and the resolved shell backend. This suite exercises that contract
//! through the REAL production paths (shell_backend + JobRegistry) on whichever
//! host CI runs on, so a no-op or Unix-only regression fails on that host. It is
//! deliberately not cfg-gated to Unix.

use std::time::Duration;

use agent::jobs::{JobRegistry, JobStatus};

/// The shell backend resolves on every supported host, or refuses with an
/// actionable error (never a silent reinterpretation).
#[test]
fn shell_backend_resolves_or_refuses_cleanly() {
    match agent::shell_backend::resolve() {
        Ok(runtime) => {
            // A resolved runtime names a real dialect and builds argv.
            assert!(!runtime.dialect().is_empty());
            let args = runtime.command_args("echo conformance");
            assert!(!args.is_empty());
        }
        Err(error) => {
            // A refusal must be actionable (name the missing runtime / override).
            assert!(
                error.contains("Bash") || error.contains("BSH_BASH_BIN"),
                "refusal must be actionable: {error}"
            );
        }
    }
}

/// The background-job lifecycle contract: a registered process is killed
/// through its ownership boundary and the kill is CONFIRMED, on every host.
#[test]
fn job_kill_confirms_process_tree_termination() {
    // Spawn a real long-lived process inside its own ownership boundary, the
    // same way the shell tool spawns a background job.
    let mut command = std::process::Command::new(if cfg!(windows) { "cmd" } else { "sh" });
    if cfg!(windows) {
        command.arg("/C").arg("ping -n 30 127.0.0.1 >NUL");
    } else {
        command.arg("-c").arg("sleep 30");
    }
    let mut owned = hostkit::proc::OwnedChild::spawn(&mut command).expect("spawn");
    let pid = owned.id();

    let registry = JobRegistry::new();
    let id = registry.begin_in("conformance", "bash", "sleep 30", std::path::PathBuf::new());
    registry.set_pid(&id, pid);
    assert!(hostkit::proc::pid_alive(pid), "the job process is alive");

    // Kill through the registry: it must terminate the tree and confirm.
    assert!(registry.kill(&id), "the kill is accepted");
    let summary = registry
        .wait_for(&id, Duration::from_secs(5))
        .expect("the job settles");
    assert_eq!(
        summary.status,
        JobStatus::Killed,
        "a confirmed kill publishes Killed, not a failure"
    );
    // The owner reaps the root (the production waiter thread does this); only
    // then is the pid fully gone, not a zombie.
    let _ = owned.child_mut().wait();
    assert!(
        !hostkit::proc::pid_alive(pid),
        "the job process is gone on every host"
    );
}

/// A naturally-exiting job settles with its exit status, not a kill.
#[test]
fn a_natural_exit_settles_with_status() {
    let mut command = std::process::Command::new(if cfg!(windows) { "cmd" } else { "sh" });
    if cfg!(windows) {
        command.arg("/C").arg("exit 0");
    } else {
        command.arg("-c").arg("exit 0");
    }
    let mut owned = hostkit::proc::OwnedChild::spawn(&mut command).expect("spawn");
    let pid = owned.id();
    let registry = JobRegistry::new();
    let id = registry.begin_in("conformance", "bash", "exit 0", std::path::PathBuf::new());
    registry.set_pid(&id, pid);
    let status = owned.child_mut().wait().expect("wait");
    registry.finish(
        &id,
        if status.success() {
            JobStatus::Succeeded { code: 0 }
        } else {
            JobStatus::Failed {
                code: status.code().unwrap_or(-1),
            }
        },
    );
    let summary = registry
        .wait_for(&id, Duration::from_secs(5))
        .expect("the job settles");
    assert_eq!(summary.status, JobStatus::Succeeded { code: 0 });
}
