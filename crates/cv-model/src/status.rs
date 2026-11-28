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
//! outcome.
//!
//! ## Operation state machine
//!
//! Long-running work is causal, not merely positional in the toast queue:
//!
//! - every long-running foreground operation mints one [`StatusOperation`]
//!   before its worker starts and carries that token through its progress,
//!   evidence, and terminal outcome;
//! - an operation is registered as live before any worker notice can arrive,
//!   is either workspace-generation scoped or shell scoped, and is closed
//!   exactly once — by an outcome naming that token or by terminal engine
//!   evidence naming that token;
//! - a notice naming a closed, retired, or unknown operation is stale and is
//!   never presented;
//! - terminal notices close only the operation they name. They never consume
//!   a newer or unrelated operation, and untagged immediate outcomes never
//!   close a tagged operation;
//! - the newest live operation owns foreground progress. Terminal product
//!   outcomes may be presented over that progress because they are facts the
//!   user asked for, but the progress operation remains live; terminal engine
//!   evidence is machinery and does not displace a newer live operation;
//! - bookkeeping and ambient engine evidence never displace live
//!   operation-owned progress/evidence or a completed outcome. They remain
//!   visible only when no foreground or outcome status owns the toast.
//!
//! Untagged progress/outcome remains as a compatibility path for immediate
//! shell commands. Untagged progress creates a shell-local legacy operation;
//! an untagged outcome may complete only that legacy operation, never a
//! causally tagged one. New long-running work must use the tagged
//! constructors instead.

use std::sync::atomic::{AtomicU64, Ordering};

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

/// Causal identity of one foreground status operation.
///
/// Tokens are minted before the operation's worker starts and travel with
/// the operation's job/notice values. Equality is identity, not coincidence
/// of text or workspace. Ordering is mint order inside this process; the
/// workbench uses it only to decide which live operation is newest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct StatusOperation(u64);

impl StatusOperation {
    /// Mint a process-unique operation token.
    pub fn mint() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::SeqCst))
    }
}

/// How long one operation remains causally relevant to the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusOperationScope {
    /// The operation belongs to one workspace generation. Project reset
    /// retires it even if its worker never reports back.
    Workspace,
    /// The operation intentionally crosses a project switch (workspace
    /// creation and migration adoption, for example).
    Shell,
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

    fn tagged(mut self, operation: StatusOperation) -> Self {
        self.operation = Some(operation);
        self
    }

    /// Progress belonging to `operation`. This may start or continue the
    /// operation's visible lifecycle, but never closes it.
    pub fn operation_progress(operation: StatusOperation, text: impl Into<String>) -> Self {
        Self::progress(text).tagged(operation)
    }

    /// The product outcome belonging to `operation`. Outcomes are terminal
    /// by definition and close exactly that operation.
    pub fn operation_outcome(operation: StatusOperation, text: impl Into<String>) -> Self {
        Self::outcome(text).tagged(operation)
    }

    /// Intermediate engine evidence belonging to `operation`: it may explain
    /// that operation, but it does not close the operation boundary.
    pub fn operation_evidence(operation: StatusOperation, text: impl Into<String>) -> Self {
        Self::engine_evidence(text).tagged(operation)
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
