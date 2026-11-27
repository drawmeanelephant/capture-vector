//! Windows process trees: each child is created suspended, assigned to its
//! own Job object with `KILL_ON_JOB_CLOSE`, and only then resumed.
//! Descendants inherit the job, so `TerminateJobObject` (or dropping the
//! handle) kills the whole tree. `CREATE_NEW_PROCESS_GROUP` keeps console
//! Ctrl events away from us.

use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenThread, ResumeThread,
    THREAD_SUSPEND_RESUME,
};

pub(crate) fn configure(cmd: &mut Command) {
    // A job can only be assigned after the process exists. Creating the
    // initial thread suspended closes the window in which a fast-forking
    // shell or engine could otherwise spawn a descendant outside the job;
    // `resume` runs only after assignment succeeds.
    cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW | CREATE_SUSPENDED);
}

/// Resume the initial thread after the process is inside its job. The pinned
/// Rust 1.99 `Child` API does not expose the main-thread handle, so recover
/// the sole thread id from the toolhelp snapshot. The child cannot create a
/// second thread while its initial thread is still suspended.
pub(crate) fn resume(child: &Child) -> std::io::Result<()> {
    let thread_id = initial_thread_id(child.id())?;
    // SAFETY: asks only for suspend/resume access to the child's initial
    // thread; the handle is checked before use and closed by the guard.
    let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) };
    if thread.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    let thread = KernelHandle(thread);
    resume_fully(thread.0)
}

fn initial_thread_id(process_id: u32) -> std::io::Result<u32> {
    // SAFETY: the snapshot handle is checked, THREADENTRY32 is sized as the
    // API requires, and the snapshot is closed by the guard on every path.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        let snapshot = KernelHandle(snapshot);
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..THREADENTRY32::default()
        };
        let mut threads = Vec::new();
        if Thread32First(snapshot.0, &mut entry) != 0 {
            loop {
                if entry.th32OwnerProcessID == process_id {
                    threads.push(entry.th32ThreadID);
                }
                if Thread32Next(snapshot.0, &mut entry) == 0 {
                    break;
                }
            }
        }
        match threads.as_slice() {
            [id] => Ok(*id),
            [] => Err(std::io::Error::other(
                "spawned child has no initial thread to resume",
            )),
            _ => Err(std::io::Error::other(
                "spawned child has multiple threads before its initial resume",
            )),
        }
    }
}

fn resume_fully(thread: HANDLE) -> std::io::Result<()> {
    loop {
        // SAFETY: `thread` is a live handle owned by the caller's guard.
        let previous = unsafe { ResumeThread(thread) };
        if previous == u32::MAX {
            return Err(std::io::Error::last_os_error());
        }
        if previous == 0 {
            return Err(std::io::Error::other(
                "spawned child's initial thread was not suspended",
            ));
        }
        if previous == 1 {
            return Ok(());
        }
    }
}

struct KernelHandle(HANDLE);

impl Drop for KernelHandle {
    fn drop(&mut self) {
        // SAFETY: every construction site owns this live kernel handle.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

pub(crate) struct Group {
    job: HANDLE,
}

// SAFETY: a job HANDLE is a kernel object reference usable from any thread.
unsafe impl Send for Group {}
unsafe impl Sync for Group {}

impl Group {
    /// Assign the freshly spawned child to a new kill-on-close job while its
    /// initial thread is still suspended. No descendant can exist yet, so
    /// every descendant spawned after `resume` inherits the job.
    pub fn adopt(child: &Child) -> std::io::Result<Self> {
        // SAFETY: FFI with valid arguments; every handle is checked and
        // closed on the error path.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 || AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) == 0 {
                let e = std::io::Error::last_os_error();
                CloseHandle(job);
                return Err(e);
            }
            Ok(Self { job })
        }
    }

    /// No polite signal exists for a windowless child; terminate is the
    /// stop. (Boris's own SIGINT/SIGTERM handling is a POSIX path.)
    pub fn terminate(&self) {
        self.kill();
    }

    pub fn kill(&self) {
        // SAFETY: `job` is a live handle owned by `self`.
        unsafe {
            TerminateJobObject(self.job, 1);
        }
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        // SAFETY: closing our own handle; KILL_ON_JOB_CLOSE reaps stragglers.
        unsafe {
            CloseHandle(self.job);
        }
    }
}
