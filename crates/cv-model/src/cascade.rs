//! Cascade dispatcher — the state machine from `cv-core::events`'s
//! cascade paragraph: build-finished → persist IR → rules re-eval →
//! findings refresh → staleness badges. Idempotent by
//! `event_id + input_hash`; replay is a no-op at every step.
//!
//! The shell slice handled here: snapshot epoch, engine handshake
//! (version gate), and degraded-state disclosure.

use crate::app_state::AppState;
use cv_core::diagnostic::{Diagnostic, Severity};
use cv_core::events::engine::{BuildPhase, EngineEvent, WatchEndReason};
use cv_core::events::shell::{DegradedReason, ShellEvent};
use cv_core::events::{AppEvent, Envelope};

/// The Boris version this build is ratified against. Compared against
/// the `hello` event's `compiler` string only — the event carries no
/// vcs revision. The revision pin is enforced earlier, at the spawn
/// gate (`cv-engine`'s `locate` requires version + vcs match before a
/// session can start); this check is the display-level recheck on the
/// only datum the event carries.
pub const WANT_ENGINE: &str = "0.9.0";

/// `boris/0.9.0` → `0.9.0`.
fn engine_version(compiler: &str) -> &str {
    let compiler = compiler
        .strip_suffix(" (cv one-shot fallback)")
        .unwrap_or(compiler);
    compiler.rsplit('/').next().unwrap_or(compiler)
}

/// The diagnostic a fatal initial build is disclosed by: the first error,
/// else the first diagnostic of any severity.
fn initial_failure(diagnostics: &[Diagnostic]) -> Option<DegradedReason> {
    let d = diagnostics
        .iter()
        .find(|d| d.severity == Severity::Error)
        .or_else(|| diagnostics.first())?;
    Some(DegradedReason::InitialBuildFailed {
        code: d.code.clone(),
        message: d.message.clone(),
    })
}

/// Apply one committed envelope to app state. Ordering and replay rules
/// are normative in `cv-core::events` docs. Returns true when changed.
pub fn dispatch(state: &mut AppState, env: &Envelope) -> bool {
    state.committed += 1;
    state.snapshot_epoch = state.snapshot_epoch.max(env.snapshot_epoch.0);
    match &env.payload {
        AppEvent::Engine(EngineEvent::WatchSessionStarted { compiler, .. }) => {
            state.engine = Some(compiler.clone());
            state.watch_live = true;
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
                    | Some(DegradedReason::InitialBuildFailed { .. })
            ) {
                state.degraded = None;
            }
        }
        AppEvent::Engine(EngineEvent::WatchSessionEnded { reason, .. }) => {
            state.watch_live = false;
            // Boris exits after a non-recoverable initial build; the dead
            // watch is its consequence, and the build failure stays the cause.
            if *reason != WatchEndReason::CleanExit
                && !matches!(
                    state.degraded,
                    Some(DegradedReason::InitialBuildFailed { .. })
                )
            {
                state.degraded = Some(DegradedReason::WatchDead);
            }
        }
        // Without diagnostics there is no cause to name; the app's
        // recovery build (`cv-app` live) discloses that case instead.
        AppEvent::Engine(EngineEvent::BuildFailed {
            phase: BuildPhase::Initial,
            diagnostics,
            recoverable: false,
        }) => {
            if let Some(reason) = initial_failure(diagnostics) {
                state.degraded = Some(reason);
            }
        }
        AppEvent::Engine(EngineEvent::BuildFinished { .. }) => {
            state.builds_finished += 1;
            if matches!(
                state.degraded,
                Some(DegradedReason::BuildFailed | DegradedReason::InitialBuildFailed { .. })
            ) {
                state.degraded = None;
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
            cause: None,
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
    fn watch_liveness_and_finished_builds_are_tracked() {
        let mut state = AppState::default();
        dispatch(&mut state, &started("boris/0.9.0"));
        assert!(state.watch_live);
        let finished = env(
            1,
            AppEvent::Engine(EngineEvent::BuildFinished {
                phase: cv_core::events::engine::BuildPhase::Rebuild,
                pages_written: 1,
                diagnostics: vec![],
                duration_ms: 1,
            }),
        );
        dispatch(&mut state, &finished);
        assert_eq!(state.builds_finished, 1);
        let ended = env(
            2,
            AppEvent::Engine(EngineEvent::WatchSessionEnded {
                reason: WatchEndReason::CleanExit,
                exit: None,
            }),
        );
        dispatch(&mut state, &ended);
        assert!(!state.watch_live);
    }

    #[test]
    fn recovery_build_clears_build_failed_and_still_counts() {
        let mut state = AppState {
            degraded: Some(DegradedReason::BuildFailed),
            ..Default::default()
        };
        let finished = env(
            1,
            AppEvent::Engine(EngineEvent::BuildFinished {
                phase: cv_core::events::engine::BuildPhase::Rebuild,
                pages_written: 1,
                diagnostics: vec![],
                duration_ms: 1,
            }),
        );
        dispatch(&mut state, &finished);
        assert_eq!(state.degraded, None);
        assert_eq!(state.builds_finished, 1);
    }

    #[test]
    fn fallback_decorated_compiler_does_not_read_as_a_version_mismatch() {
        let mut state = AppState::default();
        dispatch(&mut state, &started("boris/0.9.0 (cv one-shot fallback)"));
        assert_eq!(state.degraded, None);
        assert!(state.watch_live);
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

    fn failed_initial(diagnostics: Vec<Diagnostic>) -> Envelope {
        env(
            1,
            AppEvent::Engine(EngineEvent::BuildFailed {
                phase: BuildPhase::Initial,
                diagnostics,
                recoverable: false,
            }),
        )
    }

    fn diagnostic(code: &str, severity: Severity, message: &str) -> Diagnostic {
        Diagnostic {
            code: code.into(),
            severity,
            span: None,
            message: message.into(),
            build_version: None,
        }
    }

    fn ended(reason: WatchEndReason) -> Envelope {
        env(
            2,
            AppEvent::Engine(EngineEvent::WatchSessionEnded { reason, exit: None }),
        )
    }

    #[test]
    fn fatal_initial_build_is_disclosed_by_its_error_not_as_a_dead_watch() {
        let mut state = AppState::default();
        dispatch(&mut state, &started("boris/0.9.0"));
        dispatch(
            &mut state,
            &failed_initial(vec![
                diagnostic("WNOTE", Severity::Warning, "just a warning"),
                diagnostic("ELAYOUT", Severity::Error, "failed to load layout"),
            ]),
        );
        let want = Some(DegradedReason::InitialBuildFailed {
            code: "ELAYOUT".into(),
            message: "failed to load layout".into(),
        });
        assert_eq!(state.degraded, want);
        dispatch(&mut state, &ended(WatchEndReason::Desync));
        assert!(!state.watch_live);
        assert_eq!(state.degraded, want);
    }

    #[test]
    fn fatal_initial_build_without_diagnostics_still_reads_as_a_dead_watch() {
        let mut state = AppState::default();
        dispatch(&mut state, &started("boris/0.9.0"));
        dispatch(&mut state, &failed_initial(vec![]));
        assert_eq!(state.degraded, None);
        dispatch(&mut state, &ended(WatchEndReason::Desync));
        assert_eq!(state.degraded, Some(DegradedReason::WatchDead));
    }

    #[test]
    fn recoverable_or_rebuild_failures_do_not_claim_the_initial_build_failed() {
        let d = || vec![diagnostic("EPARENTMISSING", Severity::Error, "no parent")];
        for (phase, recoverable) in [(BuildPhase::Initial, true), (BuildPhase::Rebuild, false)] {
            let mut state = AppState::default();
            let failed = env(
                1,
                AppEvent::Engine(EngineEvent::BuildFailed {
                    phase,
                    diagnostics: d(),
                    recoverable,
                }),
            );
            dispatch(&mut state, &failed);
            assert_eq!(state.degraded, None, "{phase:?} recoverable={recoverable}");
        }
    }

    #[test]
    fn initial_build_failure_clears_on_a_green_build_or_a_new_session() {
        let finished = env(
            3,
            AppEvent::Engine(EngineEvent::BuildFinished {
                phase: BuildPhase::Initial,
                pages_written: 1,
                diagnostics: vec![],
                duration_ms: 1,
            }),
        );
        for next in [finished, started("boris/0.9.0")] {
            let mut state = AppState::default();
            dispatch(
                &mut state,
                &failed_initial(vec![diagnostic("ELAYOUT", Severity::Error, "x")]),
            );
            dispatch(&mut state, &next);
            assert_eq!(state.degraded, None);
        }
    }
}
