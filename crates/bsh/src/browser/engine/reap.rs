//! Reaping an engine process a crashed session left behind.
//!
//! # The problem this closes
//!
//! Chromium has no parent-death signal: when bsh is killed with SIGKILL (or
//! SIGTERM it cannot handle), the browser child keeps running, invisible, until
//! the machine reboots. `Drop` is a good citizen and cannot run on SIGKILL, so
//! the only reliable answer is at the NEXT start: find the child, prove it is
//! ours, and kill it.
//!
//! # How a child is identified
//!
//! Every child is launched with its own storage directory, and that directory's
//! name is the marker:
//!
//! ```text
//! bsh-browser-<pid>-<nanos>-<seq>
//! ```
//!
//! The marker is therefore in the child's argv (`--user-data-dir=<marker>`), and
//! the same name is written to a pidfile under this instance's data directory.
//! A pidfile records the pid and the marker.
//!
//! # What may be killed
//!
//! Never a process this code cannot POSITIVELY identify. A candidate is killed
//! only when ALL of these hold:
//!
//! 1. the pidfile parses, names a pid above 1, and its marker equals the
//!    marker in its file name (the pidfile index is not trusted to invent one);
//! 2. the process's parent is 1 - it really is an orphan;
//! 3. its own argv (read from the OS, not from the pidfile) contains the marker.
//!
//! Anything else is left alone and reported as skipped, so a future bug in this
//! module cannot kill an unrelated process. A process with no readable argv is
//! skipped: "cannot identify" is never "probably ours".

use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

/// The prefix every marker starts with.
pub(crate) const MARKER_PREFIX: &str = "bsh-browser";

/// How long a SIGTERM is waited for before SIGKILL.
///
/// Chromium may not exit promptly on SIGTERM, so use SIGKILL as the final
/// escalation after the graceful wait.
const TERM_GRACE: Duration = Duration::from_secs(2);

/// What a reaping pass did, so a caller can say so instead of guessing.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ReapReport {
    /// Pidfiles that were examined.
    pub(crate) examined: usize,
    /// Pids that were killed, with the marker that identified them.
    pub(crate) reaped: Vec<u32>,
    /// Pidfiles left alone and why.
    pub(crate) skipped: Vec<String>,
}

impl ReapReport {
    /// Whether anything was killed.
    pub(crate) fn reaped_any(&self) -> bool {
        !self.reaped.is_empty()
    }
}

/// The OS answers a reaper needs. Injectable, so a test can fake the parent
/// check while still killing a real process.
pub(crate) trait ProcessTable {
    /// The pid's parent.
    fn ppid(&self, pid: u32) -> Option<u32>;
    /// The pid's whole command line.
    fn argv(&self, pid: u32) -> Option<String>;
    /// Whether the pid exists.
    fn alive(&self, pid: u32) -> bool;
    /// Ask the pid to stop.
    fn terminate(&self, pid: u32) -> bool;
    /// Force the pid to stop.
    fn kill(&self, pid: u32) -> bool;
}

/// The real process table, read through `ps` and `kill`.
///
/// `ps` rather than libc: bsh does not depend on libc directly, and a check
/// that runs once per launch can afford one short-lived helper per pidfile.
/// Only pidfiles this instance's own data directory holds are ever examined, so
/// this is not a process scan.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SystemProcesses;

impl SystemProcesses {
    fn ps(pid: u32, field: &str) -> Option<String> {
        let output = Command::new("ps")
            .arg("-o")
            .arg(format!("{field}="))
            .arg("-p")
            .arg(pid.to_string())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!text.is_empty()).then_some(text)
    }
}

impl ProcessTable for SystemProcesses {
    fn ppid(&self, pid: u32) -> Option<u32> {
        Self::ps(pid, "ppid")?.parse().ok()
    }

    fn argv(&self, pid: u32) -> Option<String> {
        Self::ps(pid, "command")
    }

    fn alive(&self, pid: u32) -> bool {
        self.ppid(pid).is_some()
    }

    fn terminate(&self, pid: u32) -> bool {
        Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn kill(&self, pid: u32) -> bool {
        Command::new("kill")
            .arg("-KILL")
            .arg(pid.to_string())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
}

/// A fresh marker no other launch in this process can share.
///
/// The pid is in the name so two bsh instances never collide, and the
/// nanosecond clock covers two launches from one process in one tick (the
/// sequence number is the belt to that brace - the same trap the launch
/// suffix hit).
pub(crate) fn new_marker(seq: u64) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    format!("{MARKER_PREFIX}-{}-{nanos}-{seq}", std::process::id())
}

/// The directory an instance keeps its engine pidfiles in.
pub(crate) fn pid_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("engines")
}

/// Write the pidfile for a child, atomically enough for a reader.
pub(crate) fn write_pidfile(state_dir: &Path, pid: u32, marker: &str) -> std::io::Result<PathBuf> {
    let dir = pid_dir(state_dir);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{marker}.pid"));
    let body = format!("{pid}\n{marker}\n");
    // Write beside it and rename, so a reader never sees half a line.
    let scratch = dir.join(format!("{marker}.pid.tmp"));
    std::fs::write(&scratch, body)?;
    std::fs::rename(&scratch, &path)?;
    Ok(path)
}

/// Reap every orphaned engine this data directory knows about.
pub(crate) fn reap_orphans(state_dir: &Path, table: &dyn ProcessTable) -> ReapReport {
    let dir = pid_dir(state_dir);
    let mut report = ReapReport::default();
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(_) => return report,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("pid") {
            continue;
        }
        report.examined += 1;
        let Some(file_marker) = path.file_stem().and_then(|stem| stem.to_str()) else {
            report
                .skipped
                .push(format!("{}: no marker in the name", path.display()));
            continue;
        };
        let file_marker = file_marker.to_string();
        if !file_marker.starts_with(MARKER_PREFIX) {
            report
                .skipped
                .push(format!("{file_marker}: not a bsh marker"));
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                report
                    .skipped
                    .push(format!("{file_marker}: unreadable ({error})"));
                continue;
            }
        };
        let mut lines = text.lines();
        let pid = lines
            .next()
            .and_then(|line| line.trim().parse::<u32>().ok());
        let recorded = lines.next().map(str::trim).unwrap_or_default();
        let Some(pid) = pid.filter(|pid| *pid > 1) else {
            report.skipped.push(format!("{file_marker}: no usable pid"));
            continue;
        };
        if recorded != file_marker {
            report.skipped.push(format!(
                "{file_marker}: the pidfile names a different marker ({recorded})"
            ));
            continue;
        }
        if !table.alive(pid) {
            // Already gone: the pidfile is stale, not a process to kill.
            let _ = std::fs::remove_file(&path);
            continue;
        }
        match table.ppid(pid) {
            Some(1) => {}
            Some(parent) => {
                report.skipped.push(format!(
                    "{file_marker}: pid {pid} is not an orphan (parent {parent})"
                ));
                continue;
            }
            None => {
                report
                    .skipped
                    .push(format!("{file_marker}: pid {pid} has no readable parent"));
                continue;
            }
        }
        match table.argv(pid) {
            Some(argv) if argv.contains(&file_marker) => {}
            Some(_) => {
                report.skipped.push(format!(
                    "{file_marker}: pid {pid} does not carry the marker in its argv"
                ));
                continue;
            }
            None => {
                report
                    .skipped
                    .push(format!("{file_marker}: pid {pid} has no readable argv"));
                continue;
            }
        }
        let _ = table.terminate(pid);
        let deadline = Instant::now() + TERM_GRACE;
        while Instant::now() < deadline {
            if !table.alive(pid) {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        if table.alive(pid) {
            let _ = table.kill(pid);
        }
        let _ = std::fs::remove_file(&path);
        report.reaped.push(pid);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// A table that answers `ppid` from a map and everything else from the real
    /// OS, so a test can make a REAL child look orphaned without reparenting it.
    struct FakeParents {
        parents: HashMap<u32, Option<u32>>,
        real: SystemProcesses,
        killed: Mutex<Vec<u32>>,
    }

    impl FakeParents {
        fn new(parents: HashMap<u32, Option<u32>>) -> Self {
            Self {
                parents,
                real: SystemProcesses,
                killed: Mutex::new(Vec::new()),
            }
        }
    }

    impl ProcessTable for FakeParents {
        fn ppid(&self, pid: u32) -> Option<u32> {
            self.parents.get(&pid).copied().flatten()
        }

        fn argv(&self, pid: u32) -> Option<String> {
            self.real.argv(pid)
        }

        fn alive(&self, pid: u32) -> bool {
            self.real.alive(pid)
        }

        fn terminate(&self, pid: u32) -> bool {
            self.killed.lock().map(|mut list| list.push(pid)).ok();
            self.real.terminate(pid)
        }

        fn kill(&self, pid: u32) -> bool {
            self.real.kill(pid)
        }
    }

    fn scratch(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("bsh-reap-test-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// A real process whose argv carries `marker` when `marked` is true.
    fn spawn_sleeper(marker: &str, marked: bool) -> std::process::Child {
        let mut command = Command::new("/bin/sh");
        // A LOOP, not a single command: `sh -c "sleep 30"` execs sleep and the
        // extra argv word vanishes with the shell, which made this test look
        // like the reaper refusing a marked child. The loop keeps the shell
        // alive with its whole argv intact.
        command.arg("-c").arg("while :; do sleep 1; done");
        if marked {
            command.arg(marker);
        } else {
            command.arg("unrelated-token");
        }
        command.spawn().expect("a sleeper")
    }

    #[test]
    fn the_reaper_kills_exactly_the_marker_carrying_orphan() {
        let dir = scratch("kill");
        let owned = new_marker(1);
        let foreign = new_marker(2);
        let mut marked = spawn_sleeper(&owned, true);
        let mut unmarked = spawn_sleeper(&foreign, false);
        let marked_pid = marked.id();
        let unmarked_pid = unmarked.id();

        // Both look orphaned to the fake table; only one carries the marker in
        // its REAL argv, which is the identification that matters.
        let table = FakeParents::new(HashMap::from([
            (marked_pid, Some(1)),
            (unmarked_pid, Some(1)),
        ]));
        write_pidfile(&dir, marked_pid, &owned).expect("a pidfile");
        write_pidfile(&dir, unmarked_pid, &foreign).expect("a pidfile");

        let report = reap_orphans(&dir, &table);
        assert_eq!(report.examined, 2, "{report:?}");
        assert_eq!(report.reaped, vec![marked_pid], "{report:?}");
        assert_eq!(report.skipped.len(), 1, "{report:?}");
        assert!(
            report.skipped[0].contains("does not carry the marker"),
            "{report:?}"
        );
        // The marked child is really dead; the unrelated one still runs.
        let deadline = Instant::now() + Duration::from_secs(5);
        while marked.try_wait().expect("wait").is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            marked.try_wait().expect("wait").is_some(),
            "the marker-carrying orphan must be dead"
        );
        let _ = unmarked.kill();
        let _ = unmarked.wait();

        // The unmarked child's pidfile was deliberately NOT removed by the
        // first pass (it was never identified as ours). Once its process is
        // gone, the next pass sees a stale pidfile and clears it; the pass after
        // that has nothing to examine, so nothing can be killed twice.
        let stale = reap_orphans(&dir, &table);
        assert!(stale.reaped.is_empty(), "{stale:?}");
        assert_eq!(stale.examined, 1, "{stale:?}");
        let again = reap_orphans(&dir, &table);
        assert_eq!(again.examined, 0, "{again:?}");
    }

    #[test]
    fn a_live_child_of_a_live_parent_is_never_killed() {
        let dir = scratch("parented");
        let marker = new_marker(3);
        let mut child = spawn_sleeper(&marker, true);
        let pid = child.id();
        let table = FakeParents::new(HashMap::from([(pid, Some(std::process::id()))]));
        write_pidfile(&dir, pid, &marker).expect("a pidfile");
        let report = reap_orphans(&dir, &table);
        assert!(report.reaped.is_empty(), "{report:?}");
        assert!(
            report
                .skipped
                .iter()
                .any(|line| line.contains("not an orphan")),
            "{report:?}"
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn a_pidfile_whose_marker_does_not_match_is_not_an_identity() {
        let dir = scratch("mismatch");
        let marker = new_marker(4);
        let mut child = spawn_sleeper(&marker, true);
        let pid = child.id();
        // The file NAME is a bsh marker but its body names another one, so the
        // pidfile cannot be used to kill this pid.
        let path = pid_dir(&dir).join(format!("{marker}.pid"));
        std::fs::create_dir_all(pid_dir(&dir)).expect("a directory");
        std::fs::write(&path, format!("{pid}\nbsh-browser-someone-else\n")).expect("a pidfile");
        let table = FakeParents::new(HashMap::from([(pid, Some(1))]));
        let report = reap_orphans(&dir, &table);
        assert!(report.reaped.is_empty(), "{report:?}");
        assert!(
            report
                .skipped
                .iter()
                .any(|line| line.contains("different marker")),
            "{report:?}"
        );
        let _ = child.kill();
        let _ = child.wait();
    }
}
