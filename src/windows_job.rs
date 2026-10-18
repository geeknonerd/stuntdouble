//! One Windows Job Object per script worker: a commit-charge cap plus
//! kill-on-close orphan cleanup.
//!
//! Linux bounds a worker from the inside with `RLIMIT_AS`. Windows cannot be
//! bounded that way, so the parent creates one empty job per worker, sets
//! `JOB_OBJECT_LIMIT_PROCESS_MEMORY` to the same configured budget and
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, then assigns the freshly spawned
//! worker before it can read its job line. Closing the last handle to the job
//! terminates the worker in the kernel on every exit path, including a hard
//! kill of the server, which replaces the Unix parent sentinel.
//!
//! The job handle is stored as `std::os::windows::io::OwnedHandle`, which
//! closes it exactly once and is `Send`, so holding the guard across awaits
//! keeps the request-handler future `Send`.
//!
//! ADR 0014 (revised by #102) records this module as the crate's single
//! reviewed `unsafe` exception: the crate denies `unsafe_code`, and every
//! block below states the invariant it relies on.

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};

use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
};

/// An anonymous job object that terminates its workers when the last handle
/// closes. It owns the only handle the process creates for the job.
pub(crate) struct WorkerJob(OwnedHandle);

// The worker runs inside a request-handler future, so the guard has to stay
// `Send` even though the future holds it across awaits.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<WorkerJob>();
};

impl WorkerJob {
    /// Create an empty job holding the configured commit cap and the
    /// kill-on-close policy.
    pub(crate) fn create(memory_limit_bytes: usize) -> io::Result<Self> {
        // SAFETY: both arguments are documented as optional. Null attributes
        // ask for a handle the child cannot inherit and null name asks for an
        // anonymous job, so only the returned handle owns it. A null return is
        // the documented failure signal and is checked below.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(os_error());
        }
        // SAFETY: `handle` is a live job handle from CreateJobObjectW, and no
        // other owner exists, so moving it into an OwnedHandle makes that
        // guard the single owning reference that closes it exactly once.
        let job = unsafe { OwnedHandle::from_raw_handle(handle) };
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        info.ProcessMemoryLimit = memory_limit_bytes;
        // SAFETY: `info` is a fully initialized
        // `JOBOBJECT_EXTENDED_LIMIT_INFORMATION`, the layout the
        // `JobObjectExtendedLimitInformation` class expects, and the length
        // argument is exactly its size. The pointer stays valid for the call.
        let applied = unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                size_of_extended_limit_info(),
            )
        };
        if applied == 0 {
            // `job` drops here and closes the handle.
            return Err(os_error());
        }
        Ok(Self(job))
    }

    /// Associate a running worker with this job. The caller must pass the
    /// process handle of a worker it just spawned, and must keep this value
    /// alive until that worker is reaped.
    pub(crate) fn assign(&self, process: RawHandle) -> io::Result<()> {
        // SAFETY: `self.0` holds a live job handle exclusively owned by this
        // value, and `process` is the caller's live process handle. Handles
        // from `CreateProcess` carry the required
        // PROCESS_SET_QUOTA | PROCESS_TERMINATE rights, and the job handle
        // carries JOB_OBJECT_ASSIGN_PROCESS because this process created it.
        let assigned = unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), process) };
        if assigned == 0 {
            return Err(os_error());
        }
        Ok(())
    }
}

fn size_of_extended_limit_info() -> u32 {
    u32::try_from(std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
        .expect("extended limit information fits the class length argument")
}

fn os_error() -> io::Error {
    // SAFETY: GetLastError reads per-thread state, is always safe to call
    // right after a failed Win32 call on the same thread, and cannot fail.
    let code = unsafe { GetLastError() };
    io::Error::from_raw_os_error(code.cast_signed())
}
