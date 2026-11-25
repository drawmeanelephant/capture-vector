//! Event envelope + lane-namespaced `AppEvent` vocabulary.
//!
//! Commit boundary: event + store mutation + derived-work outbox commit
//! **atomically**; the cursor advances only after commit. Derived work is
//! idempotent by `event_id + input_hash`. Findings arriving before their
//! IR are buffered and evicted per spec.
//!
//! # Cascade paragraph (wave-0 pin — state machine)
//!
//! `build-finished` (from engine watch or one-shot run) commits in order:
//! 1. **persist IR** — the frozen `GraphDocument` replaces the in-memory
//!    `Arc` snapshot; `snapshot_epoch` bumps.
//! 2. **rules re-eval** — `cv-rules` re-evaluates the sealed `ArtifactSet`;
//!    findings are upserted by `DedupKey` (open/fixed/ignored/wontfix
//!    lifecycle is preserved across identical keys).
//! 3. **findings refresh** — the Findings mailbox emits
//!    `AuditEvent::FindingsRefreshed` carrying the diff counts.
//! 4. **staleness badges** — diagnostics older than the new `build_id` are
//!    tagged stale per the dim-or-diff-remap policy; Outputs nouns get
//!    freshness marks vs last publish receipt.
//!
//! Replay of the same `event_id + input_hash` is a no-op at every step.
//! A `snapshot` resync (watch session change) resets buffered findings
//! and restarts the cascade at step 1.

use crate::id::WorkspaceId;
use serde::{Deserialize, Serialize};

pub mod audit;
pub mod engine;
pub mod notes;
pub mod pages;
pub mod publish;
pub mod shell;

/// CV-owned resumable cursor over a normalized event stream.
/// `(engine_session_id, seq)` — a session change forces a full
/// `Engine::snapshot()` resync, never a gap-tolerant continuation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Cursor {
    pub session: SessionId,
    pub seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SessionId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EventId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SnapshotEpoch(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BuildId(pub String);

/// The unit of commit: one event, one store mutation set, one outbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub workspace: WorkspaceId,
    pub snapshot_epoch: SnapshotEpoch,
    pub build_id: Option<BuildId>,
    pub cursor: Cursor,
    pub event_id: EventId,
    pub payload: AppEvent,
}

/// Lane-namespaced payloads: additive lane changes touch only their own
/// file under `cv-core/src/events/`, so parallel lanes don't collide.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AppEvent {
    Audit(audit::AuditEvent),
    Engine(engine::EngineEvent),
    Notes(notes::NotesEvent),
    Pages(pages::PagesEvent),
    Publish(publish::PublishEvent),
    Shell(shell::ShellEvent),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::WorkspaceId;

    #[test]
    fn envelope_roundtrips() {
        let env = Envelope {
            workspace: WorkspaceId("ws".into()),
            snapshot_epoch: SnapshotEpoch(3),
            build_id: Some(BuildId("b9".into())),
            cursor: Cursor {
                session: SessionId("s1".into()),
                seq: 42,
            },
            event_id: EventId("e42".into()),
            payload: AppEvent::Shell(shell::ShellEvent::Wake),
        };
        let json = serde_json::to_string(&env).unwrap();
        let back: Envelope = serde_json::from_str(&json).unwrap();
        assert_eq!(back.cursor.seq, 42);
    }
}
