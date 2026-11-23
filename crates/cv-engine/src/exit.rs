//! Exit-status mapping. Boris codes `0/1/2/3` map verbatim to
//! [`ExitClass`]; anything a process can end with that is *not* a Boris
//! code (a Unix signal, a Windows NTSTATUS crash code, our own kill) is
//! kept distinct so it is never mistaken for a content verdict.

use cv_core::engine::ExitClass;
use serde::{Deserialize, Serialize};
use std::process::ExitStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExitOutcome {
    /// The child exited on its own with `code`.
    Exited { code: i32 },
    /// Unix only: terminated by a signal we did not send.
    Signaled { signal: i32 },
    /// We killed the process group (cancellation / stop). On Windows the
    /// exit code of a terminated job is ours, not Boris's — never mapped.
    Killed,
}

impl ExitOutcome {
    pub fn from_status(status: ExitStatus, killed_by_us: bool) -> Self {
        if killed_by_us {
            return Self::Killed;
        }
        if let Some(code) = status.code() {
            return Self::Exited { code };
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            if let Some(signal) = status.signal() {
                return Self::Signaled { signal };
            }
        }
        // `code()` is always `Some` on Windows; on Unix a status is either
        // a code or a signal. Unreachable in practice; classify as I/O.
        Self::Exited { code: 3 }
    }

    /// The Boris exit class. A Windows crash (`0xC0000005` → negative
    /// `i32`) and any code above 3 are system failures, not content.
    pub fn class(self) -> Option<ExitClass> {
        match self {
            Self::Exited { code } if (0..=3).contains(&code) => Some(ExitClass::from_code(code)),
            Self::Exited { .. } | Self::Signaled { .. } => Some(ExitClass::IoFailure),
            Self::Killed => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boris_codes_are_verbatim() {
        for (code, class) in [
            (0, ExitClass::Ok),
            (1, ExitClass::ContentFailure),
            (2, ExitClass::UsageError),
            (3, ExitClass::IoFailure),
        ] {
            assert_eq!(ExitOutcome::Exited { code }.class(), Some(class));
        }
    }

    #[test]
    fn non_boris_endings_are_not_content_verdicts() {
        // Windows access violation as surfaced by `ExitStatus::code()`.
        let ntstatus = 0xC000_0005_u32 as i32;
        assert_eq!(
            ExitOutcome::Exited { code: ntstatus }.class(),
            Some(ExitClass::IoFailure)
        );
        assert_eq!(
            ExitOutcome::Exited { code: 137 }.class(),
            Some(ExitClass::IoFailure)
        );
        assert_eq!(
            ExitOutcome::Signaled { signal: 9 }.class(),
            Some(ExitClass::IoFailure)
        );
        assert_eq!(ExitOutcome::Killed.class(), None);
    }

    #[test]
    fn real_child_statuses_map() {
        let mut ok = crate::proc::shell_command("exit 0");
        let mut usage = crate::proc::shell_command("exit 2");
        let s0 = ok.status().unwrap();
        let s2 = usage.status().unwrap();
        assert_eq!(
            ExitOutcome::from_status(s0, false).class(),
            Some(ExitClass::Ok)
        );
        assert_eq!(
            ExitOutcome::from_status(s2, false).class(),
            Some(ExitClass::UsageError)
        );
        assert_eq!(ExitOutcome::from_status(s2, true), ExitOutcome::Killed);
    }
}
