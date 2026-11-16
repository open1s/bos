//! Unix ownership: the process group is the boundary.

use std::io;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use std::os::unix::process::CommandExt;

use super::{Limits, ProcessInfo};

/// A process group led by the root pid.
#[derive(Debug)]
pub(super) struct Owner {
    pgid: u32,
    forced: AtomicBool,
}

impl Owner {
    pub(super) fn from_pid(pid: u32) -> Self {
        Self {
            pgid: pid,
            forced: AtomicBool::new(false),
        }
    }

    pub(super) fn pid(&self) -> u32 {
        self.pgid
    }

    /// The group is empty when no LIVE member remains. Zombies do not count:
    /// a killed root stays a zombie until its parent reaps it, and a
    /// termination must be confirmable before that reap.
    pub(super) fn is_empty(&self) -> bool {
        group_members(self.pgid)
            .map(|members| members.is_empty())
            // An unreadable process table cannot confirm emptiness.
            .unwrap_or(false)
    }

    pub(super) fn kill(&self, grace: Duration) -> bool {
        self.forced.store(false, Ordering::Relaxed);
        signal_group(self.pgid, libc::SIGTERM);
        if wait_empty(self.pgid, grace) {
            return true;
        }
        self.forced.store(true, Ordering::Relaxed);
        signal_group(self.pgid, libc::SIGKILL);
        wait_empty(self.pgid, super::FORCE_GRACE)
    }

    pub(super) fn was_forced(&self) -> bool {
        self.forced.load(Ordering::Relaxed)
    }
}

pub(super) fn spawn(cmd: &mut Command, limits: Limits) -> io::Result<(Child, Owner)> {
    // A fresh group whose id is the child's pid: the boundary exists from
    // the first instruction the child runs.
    cmd.process_group(0);
    if let Some(bytes) = limits.memory_bytes {
        // SAFETY: setrlimit is async-signal-safe and only touches this
        // soon-to-be-child's limits.
        unsafe {
            cmd.pre_exec(move || {
                let limit = libc::rlimit {
                    rlim_cur: bytes,
                    rlim_max: bytes,
                };
                if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let child = cmd.spawn()?;
    let owner = Owner::from_pid(child.id());
    Ok((child, owner))
}

pub(super) fn pid_alive(pid: u32) -> bool {
    // SAFETY: signal 0 performs the permission/existence check only.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if rc == 0 {
        return true;
    }
    io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn signal_group(pgid: u32, sig: libc::c_int) {
    // SAFETY: killpg only delivers a signal to the group.
    unsafe {
        libc::killpg(pgid as libc::pid_t, sig);
    }
}

fn wait_empty(pgid: u32, grace: Duration) -> bool {
    let deadline = Instant::now() + grace;
    loop {
        if group_members(pgid).map(|m| m.is_empty()).unwrap_or(false) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Live (non-zombie) members of a process group, or None when the process
/// table cannot be read.
fn group_members(pgid: u32) -> Option<Vec<u32>> {
    #[cfg(target_os = "linux")]
    {
        linux_group_members(pgid)
    }
    #[cfg(not(target_os = "linux"))]
    {
        ps_group_members(pgid)
    }
}

#[cfg(target_os = "linux")]
fn linux_group_members(pgid: u32) -> Option<Vec<u32>> {
    let mut members = Vec::new();
    let entries = std::fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        // stat is "pid (comm) state ppid pgrp ..."; comm may contain spaces
        // and parentheses, so parse after the LAST ')'.
        let Some(close) = stat.rfind(')') else {
            continue;
        };
        let rest = &stat[close + 1..];
        let mut fields = rest.split_whitespace();
        let Some(state) = fields.next() else {
            continue;
        };
        let _ppid = fields.next();
        let Some(pgrp) = fields.next().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        if pgrp == pgid && state != "Z" {
            members.push(pid);
        }
    }
    Some(members)
}

#[cfg(not(target_os = "linux"))]
fn ps_group_members(pgid: u32) -> Option<Vec<u32>> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,pgid=,state="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut members = Vec::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(pid) = fields.next().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let Some(pgrp) = fields.next().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let Some(state) = fields.next() else {
            continue;
        };
        if pgrp == pgid && !state.starts_with('Z') {
            members.push(pid);
        }
    }
    Some(members)
}

pub(super) fn process_info(pid: u32) -> Option<ProcessInfo> {
    if !pid_alive(pid) {
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        linux_process_info(pid)
    }
    #[cfg(not(target_os = "linux"))]
    {
        ps_process_info(pid)
    }
}

#[cfg(target_os = "linux")]
fn linux_process_info(pid: u32) -> Option<ProcessInfo> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close = stat.rfind(')')?;
    let rest = &stat[close + 1..];
    let mut fields = rest.split_whitespace();
    let _state = fields.next()?;
    let ppid = fields.next()?.parse::<u32>().ok();
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline"))
        .ok()
        .map(|bytes| {
            bytes
                .split(|b| *b == 0)
                .filter(|part| !part.is_empty())
                .map(|part| String::from_utf8_lossy(part).into_owned())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|line| !line.is_empty());
    Some(ProcessInfo {
        pid,
        ppid,
        command_line: cmdline,
    })
}

#[cfg(not(target_os = "linux"))]
fn ps_process_info(pid: u32) -> Option<ProcessInfo> {
    let output = Command::new("ps")
        .args(["-o", "ppid=", "-o", "command=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next()?.trim();
    let (ppid, command) = line.split_once(char::is_whitespace)?;
    Some(ProcessInfo {
        pid,
        ppid: ppid.trim().parse().ok(),
        command_line: Some(command.trim().to_string()),
    })
}

/// Unix termination owns the whole group, so a descendant walk is not needed
/// for cleanup; the empty list keeps the shared API honest.
pub(super) fn descendants(_pid: u32) -> Vec<u32> {
    Vec::new()
}
