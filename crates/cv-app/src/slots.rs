//! Composition-root registry of live engine slots by workspace root.
//! Long-running shell workers (migration adoption) reach the same
//! scheduler the watch uses instead of spawning a competing engine.

use crate::live::{EngineSlot, Scheduler};
use cv_model::workspace_key::WorkspaceKey;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

static SLOTS: OnceLock<Mutex<HashMap<WorkspaceKey, EngineSlot>>> = OnceLock::new();

fn evidence_error(e: &std::io::Error) -> String {
    // Identity evidence must not embed the path spelling in its equality
    // class: a malformed workspace is still the same malformed workspace
    // after a rename. The kind is the stable degraded identity class; the
    // workspace path is disclosed separately by the workbench.
    format!("{:?}", e.kind())
}

/// The process-local workspace-instance key shared by scheduler identity
/// and outcome ownership. `.cv` instance evidence is used together with the
/// containing directory's file identity, so renames remain one instance but
/// coexisting byte copies do not. Device+file-index and exact paths remain
/// the compatibility fallback when that evidence is unavailable.
pub fn workspace_key(root: &Path) -> WorkspaceKey {
    let file = cv_engine::file_identity::identity(root);
    match cv_store::workspace_instance::ensure(root) {
        Ok(instance) => WorkspaceKey::with_instance(root.to_path_buf(), file, instance),
        Err(e) => WorkspaceKey::with_instance_error(root.to_path_buf(), file, evidence_error(&e)),
    }
}

/// Observational key for registry lookups. Unlike [`workspace_key`], this
/// never creates `.cv`, mints evidence, or resurrects a missing workspace
/// root as a side effect of merely asking whether a slot exists.
pub fn workspace_key_lookup(root: &Path) -> WorkspaceKey {
    let file = cv_engine::file_identity::identity(root);
    match cv_store::workspace_instance::read(root) {
        Ok(Some(instance)) => WorkspaceKey::with_instance(root.to_path_buf(), file, instance),
        Ok(None) => WorkspaceKey::new(root.to_path_buf(), file),
        Err(e) => WorkspaceKey::with_instance_error(root.to_path_buf(), file, evidence_error(&e)),
    }
}

fn slots() -> &'static Mutex<HashMap<WorkspaceKey, EngineSlot>> {
    SLOTS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn register_slot(root: &Path, slot: &EngineSlot) {
    slots()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(workspace_key(root), slot.clone());
}

/// The workspace's stable slot, creating and registering it atomically
/// when this root has never been opened in the process.
pub fn slot_or_register(root: &Path) -> EngineSlot {
    slots()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(workspace_key(root))
        .or_default()
        .clone()
}

/// The workspace's engine slot (scheduler, resync, verb delivery) as
/// registered at open time, whether or not boot has finished.
pub fn slot_for(root: &Path) -> Option<EngineSlot> {
    slots()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&workspace_key_lookup(root))
        .cloned()
}

/// The workspace's scheduler once its engine has passed the gate; `None`
/// while booting or when the gate failed (disclosed in Doctor).
pub fn scheduler_for(root: &Path) -> Option<Arc<Scheduler>> {
    slots()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&workspace_key_lookup(root))
        .and_then(|slot| slot.get().cloned())
}
