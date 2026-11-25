//! Engine lane event payload. Boris stderr/exit codes arrive intact —
//! diagnostics are sacred and reach Activity as data.

use crate::diagnostic::Diagnostic;
use crate::engine::ExitClass;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EngineEvent {
    /// A build/validate/check verb finished; diagnostics attached when present.
    BuildFinished {
        phase: BuildPhase,
        pages_written: u64,
        diagnostics: Vec<Diagnostic>,
        duration_ms: u64,
    },
    BuildFailed {
        phase: BuildPhase,
        diagnostics: Vec<Diagnostic>,
        recoverable: bool,
    },
    WatchSessionStarted {
        schema: u32,
        compiler: String,
    },
    /// The watch session ended or desynced; consumers must treat the
    /// stream as terminated and wait for a snapshot resync.
    WatchSessionEnded {
        reason: WatchEndReason,
        /// Native watch child's actual process ending, when the producer is
        /// a single watch process. `None` for older recorded events and for
        /// the polling fallback, which has no one long-lived Boris process.
        #[serde(default)]
        exit: Option<WatchExit>,
    },
    /// A mutating verb completed (exit class recorded verbatim).
    VerbFinished {
        verb: String,
        exit: ExitClass,
    },
    /// A verb failed before Boris could return an exit code (spawn,
    /// cancellation, output capture, or CV-side artifact publication).
    /// The cause is engine text, never argv or secret material.
    VerbFailed {
        verb: String,
        error: String,
    },
    /// A user-requested verb ran to a Boris exit; the report is the
    /// Activity noun (see `Engine::run_reported`).
    VerbReported(VerbReport),
    /// stderr bytes that did not decode to a known event — surfaced, not
    /// swallowed (malformed lines are handled, never tolerated silently).
    MalformedLine {
        raw: String,
    },
}

/// One finished verb as the user sees it. `argv` follows the binary name
/// and never carries secrets (the engine refuses those before spawning);
/// `stderr` is verbatim, `stdout` is bounded by [`VerbReport::STDOUT_CAP`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerbReport {
    pub verb: String,
    pub argv: Vec<String>,
    pub exit: ExitClass,
    /// The raw exit code; `None` when a signal ended the process.
    pub code: Option<i32>,
    pub duration_ms: u64,
    pub stdout: String,
    pub stderr: String,
}

impl VerbReport {
    /// Bytes of stdout kept in the event: a rendered graph of a large
    /// site is a result, not a diagnostic, and the journal keeps every event.
    pub const STDOUT_CAP: usize = 256 * 1024;

    /// `stdout` cut at a char boundary under the cap, saying how much was cut.
    pub fn bounded_stdout(stdout: &str) -> String {
        if stdout.len() <= Self::STDOUT_CAP {
            return stdout.to_string();
        }
        let mut end = Self::STDOUT_CAP;
        while !stdout.is_char_boundary(end) {
            end -= 1;
        }
        format!(
            "{}\n… {} more bytes not kept",
            &stdout[..end],
            stdout.len() - end
        )
    }

    /// `boris <argv…>`, for display.
    pub fn command(&self) -> String {
        std::iter::once("boris")
            .chain(self.argv.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BuildPhase {
    Initial,
    Rebuild,
}

/// How a native watch child ended, kept distinct from unrelated verb exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WatchExit {
    Exited { code: i32 },
    Signaled { signal: i32 },
    Killed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WatchEndReason {
    CleanExit,
    Killed,
    Desync,
}
