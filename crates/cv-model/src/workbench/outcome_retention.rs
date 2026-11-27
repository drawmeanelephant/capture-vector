use crate::workspace_key::WorkspaceKey;
use cv_core::events::Envelope;
use std::collections::BTreeMap;

pub(super) const MAX_PENDING_OUTCOMES: usize = 64;
pub(super) const MAX_PENDING_OUTCOME_BYTES: usize = 1024 * 1024;

fn outcome_retention_bytes(key: &WorkspaceKey, env: &Envelope) -> usize {
    key.path().as_os_str().len() + serde_json::to_vec(env).map(|v| v.len()).unwrap_or(4096) + 64
}

/// Idempotency digest: the serialized envelope (event_id is deduped
/// separately, so this stands in for the contract's `input_hash` — a
/// reused id with a different payload or session is a new event, not a
/// replay).
pub(super) fn digest(env: &Envelope) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_vec(env).unwrap_or_default().hash(&mut h);
    h.finish()
}

/// Collapse exact republished copies before retention accounting. Event ids
/// are globally unique, so `(event id, digest)` is one logical outcome;
/// keeping the lowest causal stamp preserves the first terminal fact if a
/// copy was minted incorrectly. Reused ids with different bytes remain
/// distinct truths and continue to consume budget independently.
pub(super) fn dedupe_outcome_copies(pending: &mut Vec<(u64, WorkspaceKey, Envelope)>) {
    let mut unique: Vec<(u64, WorkspaceKey, Envelope)> = Vec::with_capacity(pending.len());
    let mut indexes: BTreeMap<(String, u64), usize> = BTreeMap::new();
    for (seq, key, env) in std::mem::take(pending) {
        let identity = (env.event_id.0.clone(), digest(&env));
        if let Some(&index) = indexes.get(&identity) {
            if seq < unique[index].0 {
                unique[index].0 = seq;
            }
        } else {
            indexes.insert(identity, unique.len());
            unique.push((seq, key, env));
        }
    }
    *pending = unique;
}

pub(super) fn enforce_outcome_retention(pending: &mut Vec<(u64, WorkspaceKey, Envelope)>) -> usize {
    let mut evicted = 0;
    let mut bytes: usize = pending
        .iter()
        .map(|(_, key, env)| outcome_retention_bytes(key, env))
        .sum();
    while pending.len() > MAX_PENDING_OUTCOMES || bytes > MAX_PENDING_OUTCOME_BYTES {
        let Some(victim) = pending
            .iter()
            .enumerate()
            .min_by_key(|(_, (seq, _, _))| *seq)
            .map(|(i, _)| i)
        else {
            break;
        };
        let (_, key, env) = &pending[victim];
        bytes = bytes.saturating_sub(outcome_retention_bytes(key, env));
        pending.remove(victim);
        evicted += 1;
    }
    evicted
}
