//! Shell status notices — outcome-first product language, with machinery
//! kept inspectable in Activity.
//!
//! ## Inventory
//!
//! Status producers fall into four classes:
//!
//! - **Outcome**: what a user-requested operation did — migration adoption
//!   and import completion/refusal, publish preflight/approval/dispatch,
//!   page/research/audit mutations, workspace creation, and command refusals.
//! - **Progress**: what a foreground operation is doing right now — creating
//!   a workspace, adoption/import inventory, rebuilding indexes, and a Boris
//!   verb starting or being cancelled.
//! - **Bookkeeping**: refreshes and presentation housekeeping — workspace
//!   boot/location notices, migration run-list reload counts, uncorrelated
//!   import refreshes, reveal/companion selection messages, and session
//!   status reloads.
//! - **Engine evidence**: Boris argv, exit classes, durations, watch
//!   identity/mode, and projection fallbacks. Activity owns this evidence;
//!   the toast may show it only when no foreground operation status is live.
//!
//! Completion text leads with the product result. Mechanical details stay in
//! Activity (or the owning inspector) rather than becoming the primary shell
//! outcome. Outcomes are sticky: progress, bookkeeping, and engine evidence
//! may replace one another, but never an outcome. A later outcome replaces
//! the earlier one because it is the newest user-requested result.

/// What kind of statement a shell status makes. The kind decides whether a
/// later notice may replace the one on screen: an outcome is the newest
/// user-requested result and is never overwritten by progress, bookkeeping,
/// or engine evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    /// The result of an operation the user asked for.
    Outcome,
    /// The foreground operation's live progress.
    Progress,
    /// Refresh or presentation housekeeping.
    Bookkeeping,
    /// Machinery evidence (argv, exit class, duration, watch identity).
    EngineEvidence,
}

impl StatusKind {
    /// Only a completed outcome is sticky for non-outcome notices.
    pub fn is_outcome(self) -> bool {
        matches!(self, Self::Outcome)
    }
}

/// One shell status line, with the product/machinery classification needed
/// to arbitrate replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusNotice {
    pub kind: StatusKind,
    pub text: String,
}

impl StatusNotice {
    pub fn new(kind: StatusKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
        }
    }

    pub fn outcome(text: impl Into<String>) -> Self {
        Self::new(StatusKind::Outcome, text)
    }

    pub fn progress(text: impl Into<String>) -> Self {
        Self::new(StatusKind::Progress, text)
    }

    pub fn bookkeeping(text: impl Into<String>) -> Self {
        Self::new(StatusKind::Bookkeeping, text)
    }

    pub fn engine_evidence(text: impl Into<String>) -> Self {
        Self::new(StatusKind::EngineEvidence, text)
    }
}
