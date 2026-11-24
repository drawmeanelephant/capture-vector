//! Cascade dispatcher — the state machine from `cv-core::events`'s
//! cascade paragraph: build-finished → persist IR → rules re-eval →
//! findings refresh → staleness badges. Idempotent by
//! `event_id + input_hash`; replay is a no-op at every step.
//!
//! The shell slice handled here: snapshot epoch, engine handshake
//! (version gate), and degraded-state disclosure.

use crate::app_state::AppState;
use cv_core::events::engine::{EngineEvent, WatchEndReason};
use cv_core::events::shell::{DegradedReason, ShellEvent};
use cv_core::events::{AppEvent, Envelope};

/// The Boris version this build is ratified against.
pub const WANT_ENGINE: &str = "0.9.0";

/// `boris/0.9.0` → `0.9.0`.
fn engine_version(compiler: &str) -> &str {
    compiler.rsplit('/').next().unwrap_or(compiler)
}

/// Apply one committed envelope to app state. Ordering and replay rules
/// are normative in `cv-core::events` docs. Returns true when changed.
pub fn dispatch(state: &mut AppState, env: &Envelope) -> bool {
    state.committed += 1;
    state.snapshot_epoch = state.snapshot_epoch.max(env.snapshot_epoch.0);
    match &env.payload {
        AppEvent::Engine(EngineEvent::WatchSessionStarted { compiler, .. }) => {
            state.engine = Some(compiler.clone());
            let found = engine_version(compiler);
            if found != WANT_ENGINE {
                state.degraded = Some(DegradedReason::EngineVersionMismatch {
                    found: found.into(),
                    want: WANT_ENGINE.into(),
                });
            } else if matches!(
                state.degraded,
                Some(DegradedReason::EngineAbsent)
                    | Some(DegradedReason::EngineVersionMismatch { .. })
                    | Some(DegradedReason::EngineBuildFailing)
                    | Some(DegradedReason::WatchDead)
            ) {
                state.degraded = None;
            }
        }
        AppEvent::Engine(EngineEvent::WatchSessionEnded { reason }) => {
            if *reason != WatchEndReason::CleanExit {
                state.degraded = Some(DegradedReason::WatchDead);
            }
        }
        AppEvent::Shell(ShellEvent::Degraded { reason }) => {
            state.degraded = Some(reason.clone());
        }
        _ => {}
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use cv_core::events::{BuildId, Cursor, EventId, SessionId, SnapshotEpoch};
    use cv_core::id::WorkspaceId;

    fn env(n: u64, payload: AppEvent) -> Envelope {
        Envelope {
            workspace: WorkspaceId("t".into()),
            snapshot_epoch: SnapshotEpoch(0),
            build_id: Some(BuildId(format!("b{n}"))),
            cursor: Cursor {
                session: SessionId("s".into()),
                seq: n,
            },
            event_id: EventId(format!("e{n}")),
            payload,
        }
    }

    fn started(compiler: &str) -> Envelope {
        env(
            0,
            AppEvent::Engine(EngineEvent::WatchSessionStarted {
                schema: 1,
                compiler: compiler.into(),
            }),
        )
    }

    #[test]
    fn matching_version_session_clears_engine_degraded_states() {
        for reason in [
            DegradedReason::EngineAbsent,
            DegradedReason::EngineVersionMismatch {
                found: "0.8.2".into(),
                want: WANT_ENGINE.into(),
            },
            DegradedReason::EngineBuildFailing,
            DegradedReason::WatchDead,
        ] {
            let mut state = AppState {
                degraded: Some(reason.clone()),
                ..Default::default()
            };
            dispatch(&mut state, &started("boris/0.9.0"));
            assert_eq!(state.degraded, None, "{reason:?} should clear");
        }
    }

    #[test]
    fn matching_version_session_keeps_unrelated_degraded_state() {
        let mut state = AppState {
            degraded: Some(DegradedReason::StoreCorrupt),
            ..Default::default()
        };
        dispatch(&mut state, &started("boris/0.9.0"));
        assert_eq!(state.degraded, Some(DegradedReason::StoreCorrupt));
    }

    #[test]
    fn version_mismatch_clears_on_later_matching_session() {
        let mut state = AppState::default();
        dispatch(&mut state, &started("boris/0.8.2"));
        assert!(matches!(
            state.degraded,
            Some(DegradedReason::EngineVersionMismatch { .. })
        ));
        dispatch(&mut state, &started("boris/0.9.0"));
        assert_eq!(state.degraded, None);
    }
}
