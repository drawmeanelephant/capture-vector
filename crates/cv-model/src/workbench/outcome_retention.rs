use crate::workspace_key::WorkspaceKey;
use cv_core::events::Envelope;

pub(super) const MAX_PENDING_OUTCOMES: usize = 64;
pub(super) const MAX_PENDING_OUTCOME_BYTES: usize = 1024 * 1024;

fn outcome_retention_bytes(key: &WorkspaceKey, env: &Envelope) -> usize {
    key.path().as_os_str().len() + serde_json::to_vec(env).map(|v| v.len()).unwrap_or(4096) + 64
}

pub(super) fn enforce_outcome_retention(pending: &mut Vec<(u64, WorkspaceKey, Envelope)>) -> usize {
    let mut evicted = 0;
    let mut bytes: usize = pending
        .iter()
        .map(|(_, key, env)| outcome_retention_bytes(key, env))
        .sum();
    while pending.len() > MAX_PENDING_OUTCOMES || bytes > MAX_PENDING_OUTCOME_BYTES {
        let Some((_, key, env)) = pending.first() else {
            break;
        };
        bytes = bytes.saturating_sub(outcome_retention_bytes(key, env));
        pending.remove(0);
        evicted += 1;
    }
    evicted
}
