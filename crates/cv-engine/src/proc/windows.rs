//! Windows process trees: each child is assigned to its own Job object
//! with `KILL_ON_JOB_CLOSE`; descendants inherit the job, so
//! `TerminateJobObject` (or dropping the handle) kills the whole tree.
//! `CREATE_NEW_PROCESS_GROUP` keeps console Ctrl events away from us.

use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW};

pub(crate) fn configure(cmd: &mut Command) {
    cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
}

pub(crate) struct Group {
    job: HANDLE,
}

// SAFETY: a job HANDLE is a kernel object reference usable from any thread.
unsafe impl Send for Group {}
unsafe impl Sync for Group {}

impl Group {
    /// Assign the freshly spawned child to a new kill-on-close job.
    /// (Grandchildren spawned in the microseconds before assignment would
    /// escape; Boris spawns none at startup, and `.bat` shims run their
    /// children only after `cmd.exe` is up.)
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
