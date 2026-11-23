//! Cascade dispatcher — the state machine from `cv-core::events`'s
//! cascade paragraph: build-finished → persist IR → rules re-eval →
//! findings refresh → staleness badges. Idempotent by
//! `event_id + input_hash`; replay is a no-op at every step.

use cv_core::events::Envelope;

/// Apply one committed envelope to app state. Ordering and replay rules
/// are normative in `cv-core::events` docs.
pub fn dispatch(_state: &mut crate::app_state::AppState, _env: &Envelope) {
    // L0 scaffold: the canary slice exercises the pattern; lane slices
    // land in `modules/<lane>/`.
}
