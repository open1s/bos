//! Process-tree ownership: spawn a process that cannot outlive its owner.
//!
//! One contract on every host:
//!
//! - [OwnedChild::spawn] puts the child inside an ownership boundary BEFORE
//!   it can run: a fresh process group on Unix, a kill-on-close Job Object on
//!   Windows (the child is created suspended, assigned, then resumed, so the
//!   assignment window cannot leak a descendant).
//! - [OwnedChild::terminate] / [TreeOwner::kill] signal the whole tree,
//!   wait a bounded grace, force, and CONFIRM the boundary is empty. The
//!   returned [Termination] says whether confirmation succeeded; a caller
//!   that cannot confirm must not report the process as killed.
//! - [TreeOwner::from_pid] adopts an already-spawned process. On Unix the
//!   spawner must have used a fresh process group (the group id is the root
//!   pid). On Windows, where a job cannot be attached after the fact without
//!   a descendant race, adoption walks the live process tree instead; it is
//!   the fallback for registries that only ever see a pid.
//!
//! Dropping an [OwnedChild] or [TreeOwner] on Windows closes the job
//! handle, which kills every process still inside it (kill-on-close). On Unix
//! dropping does NOT signal; call [TreeOwner::kill] explicitly.

use std::io;
use std::process::{Child, Command, ExitStatus};
use std::time::Duration;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
use unix as platform;
#[cfg(windows)]
use windows as platform;

/// How long a forced termination waits for the boundary to empty after the
/// kill signal, on top of the caller's graceful grace.
pub const FORCE_GRACE: Duration = Duration::from_secs(2);

/// Optional resource limits applied at spawn, inside the ownership boundary.
#[derive(Clone, Copy, Debug, Default)]
pub struct Limits {
    /// Address-space cap in bytes (Unix RLIMIT_AS, Windows job memory limit).
    pub memory_bytes: Option<u64>,
}

/// What a termination actually established.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Termination {
    /// The root process was reaped (or was already gone).
    pub root_exited: bool,
    /// The ownership boundary was confirmed empty: no live process remains
    /// in the group/job. Only a confirmed termination may be published as
    /// "killed".
    pub tree_empty: bool,
    /// The graceful signal did not empty the boundary and a force was used.
    pub forced: bool,
}

impl Termination {
    /// Whether this termination may be reported as a successful kill.
    pub fn confirmed(&self) -> bool {
        self.root_exited && self.tree_empty
    }
}

/// The ownership boundary of one process tree, without the child handle.
///
/// Keep this alive for as long as the tree may live: on Windows it holds the
/// kill-on-close job handle.
#[derive(Debug)]
pub struct TreeOwner {
    inner: platform::Owner,
}

impl TreeOwner {
    /// Adopt a process that was spawned with a fresh ownership boundary.
    ///
    /// Unix: the process must lead its own group (see [OwnedChild::spawn]);
    /// the group id is the root pid. Windows: walks the live descendant tree
    /// of the pid, because a job cannot be attached after spawn without a
    /// race.
    pub fn from_pid(pid: u32) -> Self {
        Self {
            inner: platform::Owner::from_pid(pid),
        }
    }

    /// The root process id.
    pub fn pid(&self) -> u32 {
        self.inner.pid()
    }

    /// Whether the boundary currently holds no live process.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Terminate the whole tree and confirm it is empty.
    ///
    /// Sends the platform's graceful stop (Unix SIGTERM to the group; Windows
    /// has no graceful equivalent for arbitrary processes and terminates the
    /// job at once), waits up to grace for the boundary to empty, then forces
    /// and waits [FORCE_GRACE] more. Returns whether the boundary was
    /// confirmed empty; the root child, if the caller holds one, still needs
    /// its own wait to be reaped.
    pub fn kill(&self, grace: Duration) -> bool {
        self.inner.kill(grace)
    }
}

/// A spawned child plus its ownership boundary.
#[derive(Debug)]
pub struct OwnedChild {
    child: Child,
    owner: TreeOwner,
}

impl OwnedChild {
    /// Spawn with a fresh ownership boundary and no resource limits.
    pub fn spawn(cmd: &mut Command) -> io::Result<Self> {
        Self::spawn_with(cmd, Limits::default())
    }

    /// Spawn with a fresh ownership boundary and the given limits.
    ///
    /// Fails closed: if the boundary cannot be established (Windows job
    /// creation or assignment fails), the child is killed and the error is
    /// returned rather than running unowned.
    pub fn spawn_with(cmd: &mut Command, limits: Limits) -> io::Result<Self> {
        let (child, inner) = platform::spawn(cmd, limits)?;
        Ok(Self {
            owner: TreeOwner { inner },
            child,
        })
    }

    /// The root process id.
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// The child handle, for stdio and waiting.
    pub fn child(&self) -> &Child {
        &self.child
    }

    /// The child handle, mutable.
    pub fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    /// The ownership boundary.
    pub fn owner(&self) -> &TreeOwner {
        &self.owner
    }

    /// Split into the child handle and its boundary, so a session can store
    /// both. The boundary must outlive the child.
    pub fn into_parts(self) -> (Child, TreeOwner) {
        (self.child, self.owner)
    }

    /// Non-blocking wait on the root.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    /// Blocking wait on the root.
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait()
    }

    /// Terminate the tree, reap the root, and report what was confirmed.
    pub fn terminate(&mut self, grace: Duration) -> Termination {
        let tree_empty = self.owner.kill(grace);
        let forced = self.owner.inner.was_forced();
        // Reap the root even when the boundary could not be confirmed: a
        // zombie root is still a process the caller must not leak.
        let root_exited = match self.child.try_wait() {
            Ok(Some(_)) => true,
            Ok(None) => {
                let deadline = std::time::Instant::now() + FORCE_GRACE;
                loop {
                    match self.child.try_wait() {
                        Ok(Some(_)) => break true,
                        Ok(None) => {
                            if std::time::Instant::now() >= deadline {
                                break false;
                            }
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        Err(_) => break false,
                    }
                }
            }
            Err(_) => false,
        };
        Termination {
            root_exited,
            tree_empty,
            forced,
        }
    }
}

/// Adopt a child that was created SUSPENDED into a fresh kill-on-close job.
///
/// Windows only: the caller spawned with CREATE_SUSPENDED (so the child
/// cannot have run, and cannot have spawned a descendant), and this creates
/// the job, assigns the process and resumes its initial thread. Fails closed:
/// on any error the caller must kill the child rather than run it unowned.
///
/// # Safety contract
///
/// The handle must belong to a live, suspended process the caller owns.
#[cfg(windows)]
pub fn adopt_suspended(
    pid: u32,
    process: std::os::windows::io::RawHandle,
    memory_bytes: Option<u64>,
) -> io::Result<TreeOwner> {
    let inner = platform::adopt_suspended(
        pid,
        process as windows_sys::Win32::Foundation::HANDLE,
        memory_bytes,
    )?;
    Ok(TreeOwner { inner })
}

/// Whether a pid is alive right now.
///
/// Unix: signal 0 (EPERM counts as alive). Windows: the exit code is
/// STILL_ACTIVE; an inaccessible process is conservatively reported alive.
pub fn pid_alive(pid: u32) -> bool {
    platform::pid_alive(pid)
}

/// What the host can positively establish about one process.
#[derive(Clone, Debug)]
pub struct ProcessInfo {
    /// The process id.
    pub pid: u32,
    /// The parent process id, when the host reports one.
    pub ppid: Option<u32>,
    /// The full command line, when the host allows reading it. None means
    /// "not readable", never "empty": a reaper must skip, not guess.
    pub command_line: Option<String>,
}

/// Inspect one process. Returns None when the process is gone or unreadable.
pub fn process_info(pid: u32) -> Option<ProcessInfo> {
    platform::process_info(pid)
}

/// The live descendants of a pid (children, grandchildren, ...), best effort.
///
/// Unix process-group termination does not need this (the group is the
/// boundary); it exists for the Windows adopted-pid fallback and for
/// diagnostics. A snapshot race may miss a process spawned mid-walk; callers
/// that need a guarantee must use spawn-time ownership.
pub fn descendants(pid: u32) -> Vec<u32> {
    platform::descendants(pid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt as _;
    use std::process::Stdio;

    fn sleeper_cmd(secs: &str) -> Command {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(format!("sleep {secs}"))
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd
    }

    /// A spawned child leads its own boundary, and terminating it confirms
    /// the boundary empty and reaps the root.
    #[test]
    fn spawn_owns_and_terminate_confirms() {
        let mut child = OwnedChild::spawn(&mut sleeper_cmd("30")).expect("spawn");
        let pid = child.id();
        assert!(pid_alive(pid));
        let report = child.terminate(Duration::from_millis(500));
        assert!(
            report.confirmed(),
            "termination must be confirmed: {report:?}"
        );
        assert!(!pid_alive(pid), "the root must be gone");
    }

    /// A descendant spawned by the child dies with the tree: the boundary,
    /// not the root pid, is what termination owns.
    #[test]
    fn terminate_kills_descendants() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg("sleep 30 & sleep 30")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = OwnedChild::spawn(&mut cmd).expect("spawn");
        std::thread::sleep(Duration::from_millis(200));
        let report = child.terminate(Duration::from_millis(500));
        assert!(report.confirmed(), "the group must empty: {report:?}");
    }

    /// A process that exits on its own is reaped and reported honestly.
    #[test]
    fn natural_exit_is_reported() {
        let mut child = OwnedChild::spawn(&mut sleeper_cmd("0.05")).expect("spawn");
        let status = child.wait().expect("wait");
        assert!(status.success());
        assert!(child.owner().is_empty());
    }

    /// Adopting a pid that leads its own group terminates the group.
    #[test]
    fn adopted_pid_owns_the_group() {
        let mut cmd = sleeper_cmd("30");
        cmd.process_group(0);
        let mut child = cmd.spawn().expect("spawn");
        let pid = child.id();
        let owner = TreeOwner::from_pid(pid);
        assert!(!owner.is_empty());
        assert!(owner.kill(Duration::from_millis(500)));
        let _ = child.wait();
        assert!(owner.is_empty());
    }

    /// Inspecting a live process reports its command line; a gone pid does not.
    #[test]
    fn process_info_reads_the_command_line() {
        let mut child = OwnedChild::spawn(&mut sleeper_cmd("5")).expect("spawn");
        let info = process_info(child.id()).expect("live process is inspectable");
        assert_eq!(info.pid, child.id());
        assert!(
            info.command_line.as_deref().unwrap_or("").contains("sleep"),
            "command line must be readable: {info:?}"
        );
        let _ = child.terminate(Duration::from_millis(500));
        assert!(!pid_alive(child.id()));
    }

    /// Cross-platform conformance guard (issue #313): the shared process
    /// contract must hold on EVERY host, with no Unix-only assumption and no
    /// non-Unix no-op. This test is deliberately NOT cfg-gated: reintroducing
    /// a no-op platform implementation fails it on that platform's CI lane.
    #[test]
    fn conformance_process_lifecycle_on_every_host() {
        let mut child = OwnedChild::spawn(&mut sleeper_cmd("30")).expect("spawn");
        let pid = child.id();
        assert!(
            pid_alive(pid),
            "a live child must be detected on every host"
        );
        let report = child.terminate(Duration::from_millis(500));
        assert!(
            report.confirmed(),
            "termination must be confirmed: {report:?}"
        );
        assert!(
            !pid_alive(pid),
            "a terminated child must be gone on every host"
        );
    }
}
