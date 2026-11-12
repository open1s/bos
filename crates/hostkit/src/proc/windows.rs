//! Windows ownership: a kill-on-close Job Object is the boundary.
//!
//! The child is created SUSPENDED, assigned to the job, and only then
//! resumed, so it cannot spawn a descendant before the boundary exists.
//! Dropping the job handle terminates everything still inside it.

use std::io;
use std::mem::{size_of, zeroed};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, STILL_ACTIVE, UNICODE_STRING};
use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, Thread32First, Thread32Next,
    PROCESSENTRY32W, TH32CS_SNAPPROCESS, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicProcessIdList,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
    TerminateJobObject, JOBOBJECT_BASIC_PROCESS_ID_LIST, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, OpenThread, ResumeThread, CREATE_SUSPENDED, PEB,
    PROCESS_BASIC_INFORMATION, PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_TERMINATE, PROCESS_VM_READ, RTL_USER_PROCESS_PARAMETERS, THREAD_SUSPEND_RESUME,
};

use super::{Limits, ProcessInfo, FORCE_GRACE};

/// A kill-on-close job handle.
#[derive(Debug)]
pub(super) struct JobObject(HANDLE);

// Job handles are process-wide and safe to share across threads.
unsafe impl Send for JobObject {}
unsafe impl Sync for JobObject {}

impl JobObject {
    fn new(memory_bytes: Option<u64>) -> io::Result<Self> {
        // SAFETY: null name/attributes are the documented anonymous form;
        // the handle is checked before use.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the structure is POD; zero is its documented empty state.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if let Some(bytes) = memory_bytes {
            limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
            limits.ProcessMemoryLimit = bytes as usize;
        }
        // SAFETY: handle is live; pointer/size describe limits.
        let ok = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            let err = io::Error::last_os_error();
            // SAFETY: the handle was created above and is not yet in use.
            unsafe { CloseHandle(handle) };
            return Err(err);
        }
        Ok(Self(handle))
    }

    fn assign(&self, process: HANDLE) -> io::Result<()> {
        // SAFETY: both handles are live; the process is suspended.
        let ok = unsafe { AssignProcessToJobObject(self.0, process) };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn terminate(&self) {
        // SAFETY: the handle is owned by this value.
        unsafe { TerminateJobObject(self.0, 1) };
    }

    /// Live processes in the job, or None when the query fails.
    fn process_count(&self) -> Option<u32> {
        const CAP: usize = 4096;
        let bytes = size_of::<JOBOBJECT_BASIC_PROCESS_ID_LIST>() + (CAP - 1) * size_of::<usize>();
        let mut buf = vec![0u8; bytes];
        // SAFETY: buf is large enough for the header plus CAP ids.
        let ok = unsafe {
            QueryInformationJobObject(
                self.0,
                JobObjectBasicProcessIdList,
                buf.as_mut_ptr().cast(),
                bytes as u32,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return None;
        }
        // SAFETY: the query wrote a valid header into buf.
        let list: &JOBOBJECT_BASIC_PROCESS_ID_LIST = unsafe { &*buf.as_ptr().cast() };
        Some(list.NumberOfProcessIdsInList)
    }
}

impl Drop for JobObject {
    fn drop(&mut self) {
        // Kill-on-close: closing the last handle terminates every process
        // still in the job.
        // SAFETY: the handle is owned here and closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

/// The ownership boundary: a job for spawn-owned trees, a pid walk for
/// adopted ones.
#[derive(Debug)]
pub(super) enum Owner {
    Job {
        job: JobObject,
        pid: u32,
        forced: AtomicBool,
    },
    Adopted {
        pid: u32,
        forced: AtomicBool,
    },
}

impl Owner {
    pub(super) fn from_pid(pid: u32) -> Self {
        Self::Adopted {
            pid,
            forced: AtomicBool::new(false),
        }
    }

    pub(super) fn pid(&self) -> u32 {
        match self {
            Self::Job { pid, .. } | Self::Adopted { pid, .. } => *pid,
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        match self {
            Self::Job { job, .. } => job.process_count() == Some(0),
            Self::Adopted { pid, .. } => !pid_alive(*pid) && descendants(*pid).is_empty(),
        }
    }

    pub(super) fn kill(&self, grace: Duration) -> bool {
        match self {
            Self::Job { job, forced, .. } => {
                // Windows has no graceful stop for arbitrary processes; the
                // job termination IS the stop, and the grace bounds the
                // confirmation wait.
                forced.store(true, Ordering::Relaxed);
                job.terminate();
                wait_empty(|| job.process_count() == Some(0), grace + FORCE_GRACE)
            }
            Self::Adopted { pid, forced } => {
                forced.store(true, Ordering::Relaxed);
                let mut targets = descendants(*pid);
                targets.push(*pid);
                for target in &targets {
                    terminate_pid(*target);
                }
                wait_empty(
                    || targets.iter().all(|p| !pid_alive(*p)),
                    grace + FORCE_GRACE,
                )
            }
        }
    }

    pub(super) fn was_forced(&self) -> bool {
        match self {
            Self::Job { forced, .. } | Self::Adopted { forced, .. } => {
                forced.load(Ordering::Relaxed)
            }
        }
    }
}

fn wait_empty(mut empty: impl FnMut() -> bool, grace: Duration) -> bool {
    let deadline = Instant::now() + grace;
    loop {
        if empty() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub(super) fn spawn(cmd: &mut Command, limits: Limits) -> io::Result<(Child, Owner)> {
    let job = JobObject::new(limits.memory_bytes)?;
    // Created suspended: the child cannot run (and so cannot spawn a
    // descendant) until it is inside the job.
    cmd.creation_flags(CREATE_SUSPENDED);
    let mut child = cmd.spawn()?;
    let raw = child.as_raw_handle() as HANDLE;
    if let Err(err) = job.assign(raw) {
        // Fail closed: an unowned child must not run.
        let _ = child.kill();
        let _ = child.wait();
        return Err(err);
    }
    if let Err(err) = resume_main_thread(child.id()) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(err);
    }
    let owner = Owner::Job {
        job,
        pid: child.id(),
        forced: AtomicBool::new(false),
    };
    Ok((child, owner))
}

/// Resume the suspended initial thread of a freshly created process.
fn resume_main_thread(pid: u32) -> io::Result<()> {
    // SAFETY: a thread snapshot is a read-only enumeration handle.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot.is_null() || snapshot == (-1isize as HANDLE) {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the structure is POD with its size field set.
    let mut entry: THREADENTRY32 = unsafe { zeroed() };
    entry.dwSize = size_of::<THREADENTRY32>() as u32;
    let mut found = None;
    // SAFETY: snapshot is live; entry is sized.
    let mut ok = unsafe { Thread32First(snapshot, &mut entry) };
    while ok != 0 {
        if entry.th32OwnerProcessID == pid {
            found = Some(entry.th32ThreadID);
            break;
        }
        // SAFETY: same snapshot and entry.
        ok = unsafe { Thread32Next(snapshot, &mut entry) };
    }
    // SAFETY: the snapshot handle is owned here.
    unsafe { CloseHandle(snapshot) };
    let tid = found.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "suspended process has no initial thread",
        )
    })?;
    // SAFETY: the thread belongs to the suspended child and cannot exit
    // before it is resumed.
    let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, tid) };
    if thread.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the thread handle is live.
    let prev = unsafe { ResumeThread(thread) };
    // SAFETY: the thread handle is owned here.
    unsafe { CloseHandle(thread) };
    if prev == u32::MAX {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn terminate_pid(pid: u32) {
    // SAFETY: opening with terminate rights only; failure is ignored because
    // the confirmation wait reports whether the process actually died.
    let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
    if !handle.is_null() {
        // SAFETY: the handle is live.
        unsafe {
            windows_sys::Win32::System::Threading::TerminateProcess(handle, 1);
            CloseHandle(handle);
        }
    }
}

/// Create the job, assign the suspended process, resume its initial thread.
pub(super) fn adopt_suspended(
    pid: u32,
    process: HANDLE,
    memory_bytes: Option<u64>,
) -> io::Result<Owner> {
    let job = JobObject::new(memory_bytes)?;
    if let Err(err) = job.assign(process) {
        return Err(err);
    }
    if let Err(err) = resume_main_thread(pid) {
        return Err(err);
    }
    Ok(Owner::Job {
        job,
        pid,
        forced: AtomicBool::new(false),
    })
}

pub(super) fn pid_alive(pid: u32) -> bool {
    // SAFETY: query-limited rights only.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        // Access denied means the process exists but is protected; a missing
        // pid fails with a different error. Conservatively report alive when
        // the failure is not "not found".
        let err = io::Error::last_os_error().raw_os_error();
        return err != Some(87); // ERROR_INVALID_PARAMETER
    }
    let mut code: u32 = 0;
    // SAFETY: handle is live; code is an out parameter.
    let ok = unsafe { GetExitCodeProcess(handle, &mut code) };
    // SAFETY: the handle is owned here.
    unsafe { CloseHandle(handle) };
    if ok == 0 {
        return true;
    }
    code == STILL_ACTIVE as u32
}

pub(super) fn process_info(pid: u32) -> Option<ProcessInfo> {
    let ppid = process_ppid(pid);
    if ppid.is_none() && !pid_alive(pid) {
        return None;
    }
    Some(ProcessInfo {
        pid,
        ppid,
        command_line: read_command_line(pid),
    })
}

/// Parent pid from a process snapshot.
fn process_ppid(pid: u32) -> Option<u32> {
    // SAFETY: a process snapshot is a read-only enumeration handle.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot.is_null() || snapshot == (-1isize as HANDLE) {
        return None;
    }
    // SAFETY: POD with size set.
    let mut entry: PROCESSENTRY32W = unsafe { zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let mut found = None;
    // SAFETY: snapshot live, entry sized.
    let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) };
    while ok != 0 {
        if entry.th32ProcessID == pid {
            found = Some(entry.th32ParentProcessID);
            break;
        }
        // SAFETY: same snapshot and entry.
        ok = unsafe { Process32NextW(snapshot, &mut entry) };
    }
    // SAFETY: owned here.
    unsafe { CloseHandle(snapshot) };
    found
}

/// Read the target process's command line through its PEB.
///
/// Requires query + VM-read rights; returns None when the process is
/// protected, gone, or raced — never a guess.
fn read_command_line(pid: u32) -> Option<String> {
    // SAFETY: query and read rights only.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let result = read_command_line_inner(handle);
    // SAFETY: owned here.
    unsafe { CloseHandle(handle) };
    result
}

fn read_command_line_inner(handle: HANDLE) -> Option<String> {
    // SAFETY: POD out parameter.
    let mut info: PROCESS_BASIC_INFORMATION = unsafe { zeroed() };
    // SAFETY: handle live; info sized.
    let status = unsafe {
        NtQueryInformationProcess(
            handle,
            ProcessBasicInformation,
            (&mut info as *mut PROCESS_BASIC_INFORMATION).cast(),
            size_of::<PROCESS_BASIC_INFORMATION>() as u32,
            std::ptr::null_mut(),
        )
    };
    if status < 0 || info.PebBaseAddress.is_null() {
        return None;
    }
    // SAFETY: POD out parameter.
    let mut peb: PEB = unsafe { zeroed() };
    // SAFETY: reading the target's PEB at the address the kernel reported.
    let ok = unsafe {
        ReadProcessMemory(
            handle,
            info.PebBaseAddress.cast(),
            (&mut peb as *mut PEB).cast(),
            size_of::<PEB>(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 || peb.ProcessParameters.is_null() {
        return None;
    }
    // SAFETY: POD out parameter.
    let mut params: RTL_USER_PROCESS_PARAMETERS = unsafe { zeroed() };
    // SAFETY: reading the parameter block the PEB points at.
    let ok = unsafe {
        ReadProcessMemory(
            handle,
            peb.ProcessParameters.cast(),
            (&mut params as *mut RTL_USER_PROCESS_PARAMETERS).cast(),
            size_of::<RTL_USER_PROCESS_PARAMETERS>(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return None;
    }
    let cmd: UNICODE_STRING = params.CommandLine;
    let len = cmd.Length as usize;
    if len == 0 || len > 64 * 1024 || cmd.Buffer.is_null() {
        return None;
    }
    let mut buf = vec![0u16; len / 2];
    // SAFETY: buf holds len bytes; the buffer address came from the target.
    let ok = unsafe {
        ReadProcessMemory(
            handle,
            cmd.Buffer.cast(),
            buf.as_mut_ptr().cast(),
            len,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buf).trim().to_string())
}

pub(super) fn descendants(pid: u32) -> Vec<u32> {
    // SAFETY: read-only enumeration handle.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot.is_null() || snapshot == (-1isize as HANDLE) {
        return Vec::new();
    }
    let mut children: Vec<(u32, u32)> = Vec::new();
    // SAFETY: POD with size set.
    let mut entry: PROCESSENTRY32W = unsafe { zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: snapshot live, entry sized.
    let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) };
    while ok != 0 {
        children.push((entry.th32ParentProcessID, entry.th32ProcessID));
        // SAFETY: same snapshot and entry.
        ok = unsafe { Process32NextW(snapshot, &mut entry) };
    }
    // SAFETY: owned here.
    unsafe { CloseHandle(snapshot) };
    let mut out = Vec::new();
    let mut queue = vec![pid];
    while let Some(parent) = queue.pop() {
        for (ppid, child) in &children {
            if *ppid == parent && *child != pid {
                out.push(*child);
                queue.push(*child);
            }
        }
    }
    out
}
