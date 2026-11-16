//! Windows Job Object enforcement for extension child processes.
//!
//! A Job Object is the Windows equivalent of the Unix process group plus
//! RLIMIT_AS: every child is assigned to a job that caps its committed memory
//! and kills the whole job when the handle closes. The handle lives with the
//! runtime, so stopping a runtime terminates the child and any descendants it
//! spawned — the same guarantee `killpg` provides on Unix.
//!
//! `max_execution_ms` is deliberately not turned into a job CPU limit for the
//! same reason RLIMIT_CPU is unlimited on Unix: it is a per-call wall-clock
//! deadline, not a process-lifetime budget.

use std::io;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
};

/// One job object owning a child's resource limits.
pub(crate) struct JobObject(HANDLE);

// The handle is used from whichever thread owns the runtime; Windows job
// handles are process-wide and safe to share.
unsafe impl Send for JobObject {}
unsafe impl Sync for JobObject {}

impl JobObject {
    /// Create a job capped at `memory_bytes` committed memory.
    pub(crate) fn new(memory_bytes: u64) -> io::Result<Self> {
        // SAFETY: both arguments are documented null values (no name, no
        // attributes); the returned handle is checked before use.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION =
            // SAFETY: the structure is POD; zero is its documented empty state.
            unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        limits.ProcessMemoryLimit = memory_bytes as usize;
        // SAFETY: the handle is live and the pointer/size describe `limits`.
        let ok = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            let error = io::Error::last_os_error();
            // SAFETY: the handle was created above and is not in use.
            unsafe { CloseHandle(handle) };
            return Err(error);
        }
        Ok(Self(handle))
    }

    /// Assign an already-open process handle to this job.
    pub(crate) fn assign(&self, process: HANDLE) -> io::Result<()> {
        // SAFETY: both handles are live and owned by the caller/session.
        let ok = unsafe { AssignProcessToJobObject(self.0, process) };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

impl Drop for JobObject {
    fn drop(&mut self) {
        // Closing the last handle terminates every process still in the job
        // because JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE is set.
        // SAFETY: the handle is owned by this value and closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}
