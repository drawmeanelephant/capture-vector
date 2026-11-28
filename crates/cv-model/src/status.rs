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
//! outcome. Replacement is a small foreground-operation state machine rather
//! than a fixed priority order:
//!
//! - live progress always starts or continues the newest foreground
//!   operation, so it may replace an older operation's outcome;
//! - an outcome completes that foreground operation and replaces anything;
//! - while progress is live, bookkeeping and ambient engine evidence stay out
//!   of the toast (their detail remains in Activity); terminal engine evidence
//!   carrying the live operation's causal token may complete that operation;
//! - a completed outcome holds against ambient bookkeeping and engine
//!   evidence until another foreground operation starts; evidence belonging
//!   to that new operation may supersede the older outcome;
//! - bookkeeping and engine evidence replace one another when no foreground
//!   operation is live.
//!
/// What kind of statement a shell status makes. The kind decides whether a
/// later notice may replace the one on screen.
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

/// Causal identity of one foreground status operation. Workers that report
/// operation-bound evidence carry this token back with the notice; ambient
/// notices have no token and can never claim the operation's boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusOperation(u64);

impl StatusOperation {
    pub(crate) fn new(id: u64) -> Self {
        Self(id)
    }
}

/// One shell status line, with the product/machinery classification needed
/// to arbitrate replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusNotice {
    pub kind: StatusKind,
    pub text: String,
    pub(crate) operation: Option<StatusOperation>,
    /// Whether operation-bound engine evidence is the operation's terminal
    /// notice. Intermediate evidence may explain the operation but must not
    /// close its boundary. Meaningless for non-evidence kinds.
    pub(crate) terminal_evidence: bool,
}

impl StatusNotice {
    pub fn new(kind: StatusKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
            operation: None,
            terminal_evidence: false,
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

    /// Intermediate engine evidence belonging to `operation`: it may explain
    /// that operation, but it does not close the operation boundary.
    pub fn operation_evidence(operation: StatusOperation, text: impl Into<String>) -> Self {
        let mut notice = Self::engine_evidence(text);
        notice.operation = Some(operation);
        notice
    }

    /// Terminal engine evidence belonging to `operation`. This is for boot or
    /// watch outcomes whose truthful terminal fact is machinery evidence rather
    /// than a product outcome; it closes the operation boundary it names.
    pub fn terminal_operation_evidence(
        operation: StatusOperation,
        text: impl Into<String>,
    ) -> Self {
        let mut notice = Self::operation_evidence(operation, text);
        notice.terminal_evidence = true;
        notice
    }
}
