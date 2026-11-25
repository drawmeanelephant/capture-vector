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
    /// stderr bytes that did not decode to a known event — surfaced, not
    /// swallowed (malformed lines are handled, never tolerated silently).
    MalformedLine {
        raw: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BuildPhase {
    Initial,
    Rebuild,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WatchEndReason {
    CleanExit,
    Killed,
    Desync,
}
