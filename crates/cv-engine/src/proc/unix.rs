//! POSIX process groups: the child leads a fresh group (`setpgid(0,0)`),
//! so `kill(-pgid, …)` reaches every descendant that didn't escape it.

use std::os::unix::process::CommandExt;
use std::process::{Child, Command};

pub(crate) fn configure(cmd: &mut Command) {
    cmd.process_group(0);
}

pub(crate) struct Group {
    pgid: libc::pid_t,
}

impl Group {
    pub fn adopt(child: &Child) -> std::io::Result<Self> {
        Ok(Self {
            pgid: child.id() as libc::pid_t,
        })
    }

    fn signal(&self, sig: libc::c_int) {
        // ESRCH (group already gone) is the expected success case.
        // SAFETY: plain syscall on a pgid we created; no memory involved.
        unsafe {
            libc::kill(-self.pgid, sig);
        }
    }

    pub fn terminate(&self) {
        self.signal(libc::SIGTERM);
    }

    pub fn kill(&self) {
        self.signal(libc::SIGKILL);
    }
}
